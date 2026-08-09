use anyhow::Context;
use ring::digest;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::db::Pool;

use super::{load_file_touch_context, load_projection_source_events};
use crate::ingest::timeline::{
    build_session_projection, load_session_events, SessionEventFilter, StoredEvent,
    StoredOperationProjection, StoredTurnProjection,
};

pub async fn rebuild_session_projection(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<usize> {
    let events = load_projection_source_events(pool, session_uuid)
        .await
        .context("load canonical projection events")?;
    let file_context = load_file_touch_context(pool, session_uuid).await?;
    let projected = build_session_projection(&events, file_context.as_ref());

    let mut tx = pool.begin().await.context("begin projection tx")?;
    clear_session_projection(&mut tx, session_uuid).await?;
    let expected_source_keys = insert_projection_rows(&mut tx, session_uuid, &projected).await?;
    reconcile_operation_embedding_sources(&mut tx, session_uuid, None, &expected_source_keys)
        .await?;
    refresh_timeline_session_state(&mut tx, session_uuid).await?;
    tx.commit().await.context("commit projection tx")?;
    Ok(projected.len())
}

pub async fn rebuild_session_projection_after_insert(
    pool: &Pool,
    session_uuid: Uuid,
    first_inserted_offset: i64,
) -> anyhow::Result<usize> {
    if session_has_descendants(pool, session_uuid).await? {
        return rebuild_session_projection(pool, session_uuid).await;
    }

    let Some(anchor) = projection_rebuild_anchor(pool, session_uuid, first_inserted_offset).await?
    else {
        return rebuild_session_projection(pool, session_uuid).await;
    };

    let events = load_direct_session_events_from(pool, session_uuid, anchor.turn_id)
        .await
        .context("load canonical projection suffix events")?;
    if events.is_empty() {
        return rebuild_session_projection(pool, session_uuid).await;
    }

    let file_context = load_file_touch_context(pool, session_uuid).await?;
    let mut projected = build_session_projection(&events, file_context.as_ref());
    for (idx, turn) in projected.iter_mut().enumerate() {
        turn.turn_ord = anchor.turn_ord + idx as i32;
    }

    let mut tx = pool
        .begin()
        .await
        .context("begin incremental projection tx")?;
    clear_session_projection_from(&mut tx, session_uuid, anchor.turn_id).await?;
    let expected_source_keys = insert_projection_rows(&mut tx, session_uuid, &projected).await?;
    reconcile_operation_embedding_sources(
        &mut tx,
        session_uuid,
        Some(anchor.turn_id),
        &expected_source_keys,
    )
    .await?;
    refresh_timeline_session_state(&mut tx, session_uuid).await?;
    tx.commit()
        .await
        .context("commit incremental projection tx")?;
    Ok(projected.len())
}

#[derive(Debug, Clone, Copy)]
struct ProjectionAnchor {
    turn_id: i64,
    turn_ord: i32,
}

async fn projection_rebuild_anchor(
    pool: &Pool,
    session_uuid: Uuid,
    first_inserted_offset: i64,
) -> anyhow::Result<Option<ProjectionAnchor>> {
    let row: Option<(i64, i32)> = sqlx::query_as(
        "SELECT turn_id, turn_ord \
           FROM timeline_turns \
          WHERE session_uuid = $1 AND turn_id <= $2 \
          ORDER BY turn_id DESC \
          LIMIT 1",
    )
    .bind(session_uuid)
    .bind(first_inserted_offset)
    .fetch_optional(pool)
    .await
    .context("load projection rebuild anchor")?;

    Ok(row.map(|(turn_id, turn_ord)| ProjectionAnchor { turn_id, turn_ord }))
}

async fn session_has_descendants(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 \
               FROM claude_sessions \
              WHERE parent_session_uuid = $1 \
         )",
    )
    .bind(session_uuid)
    .fetch_one(pool)
    .await
    .context("check projection descendants")
}

async fn load_direct_session_events_from(
    pool: &Pool,
    session_uuid: Uuid,
    from_offset: i64,
) -> anyhow::Result<Vec<StoredEvent>> {
    load_session_events(
        pool,
        session_uuid,
        &SessionEventFilter {
            after: Some(from_offset.saturating_sub(1)),
            limit: None,
            kind: None,
        },
    )
    .await
    .context("load direct session events from offset")
}

async fn clear_session_projection(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
) -> anyhow::Result<()> {
    for table in [
        "timeline_file_touches",
        "timeline_activity_signals",
        "timeline_operations",
        "timeline_turns",
    ] {
        let sql = format!("DELETE FROM {table} WHERE session_uuid = $1");
        sqlx::query(&sql)
            .bind(session_uuid)
            .execute(&mut **tx)
            .await
            .with_context(|| format!("clear {table}"))?;
    }
    Ok(())
}

async fn clear_session_projection_from(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    from_turn_id: i64,
) -> anyhow::Result<()> {
    for table in [
        "timeline_file_touches",
        "timeline_activity_signals",
        "timeline_operations",
        "timeline_turns",
    ] {
        let sql = format!("DELETE FROM {table} WHERE session_uuid = $1 AND turn_id >= $2");
        sqlx::query(&sql)
            .bind(session_uuid)
            .bind(from_turn_id)
            .execute(&mut **tx)
            .await
            .with_context(|| format!("clear {table} projection suffix"))?;
    }
    Ok(())
}

async fn insert_projection_rows(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    projected: &[StoredTurnProjection],
) -> anyhow::Result<Vec<String>> {
    let mut expected_source_keys = Vec::new();
    for turn in projected {
        insert_projected_turn(tx, session_uuid, turn, &mut expected_source_keys).await?;
    }
    Ok(expected_source_keys)
}

async fn reconcile_operation_embedding_sources(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    from_turn_id: Option<i64>,
    expected_source_keys: &[String],
) -> anyhow::Result<()> {
    sqlx::query_scalar::<_, i64>(
        "WITH stale AS ( \
             SELECT source_key \
               FROM retrieval_embedding_sources \
              WHERE session_uuid = $1 \
                AND source_family IN ('operation_call', 'operation_result') \
                AND ($2::BIGINT IS NULL OR turn_id >= $2) \
                AND deleted_at IS NULL \
                AND NOT (source_key = ANY($3::TEXT[])) \
        ), removed_embeddings AS ( \
             DELETE FROM retrieval_embeddings re \
              USING stale \
              WHERE re.source_key = stale.source_key \
              RETURNING re.source_key \
        ), removed_sources AS ( \
             UPDATE retrieval_embedding_sources s \
                SET index_status = 'deleted', deleted_at = NOW(), updated_at = NOW() \
               FROM stale \
              WHERE s.source_key = stale.source_key \
              RETURNING s.id \
        ) \
        SELECT COUNT(*)::BIGINT FROM removed_sources",
    )
    .bind(session_uuid)
    .bind(from_turn_id)
    .bind(expected_source_keys)
    .fetch_one(&mut **tx)
    .await
    .context("reconcile operation retrieval sources")?;
    Ok(())
}

async fn refresh_timeline_session_state(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO timeline_session_state \
             (session_uuid, revision, total_event_count, turn_count, latest_turn_id, latest_event_at, updated_at) \
         SELECT $1, 1, COALESCE(SUM(event_count), 0)::BIGINT, COUNT(turn_id)::BIGINT, \
                MAX(turn_id), MAX(end_timestamp), NOW() \
           FROM timeline_turns \
          WHERE session_uuid = $1 \
         ON CONFLICT (session_uuid) DO UPDATE SET \
             revision = timeline_session_state.revision + 1, \
             total_event_count = EXCLUDED.total_event_count, \
             turn_count = EXCLUDED.turn_count, \
             latest_turn_id = EXCLUDED.latest_turn_id, \
             latest_event_at = EXCLUDED.latest_event_at, \
             updated_at = NOW()",
    )
    .bind(session_uuid)
    .execute(&mut **tx)
    .await
    .context("refresh timeline session state")?;
    Ok(())
}

async fn insert_projected_turn(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    turn: &StoredTurnProjection,
    expected_source_keys: &mut Vec<String>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO timeline_turns \
             (session_uuid, turn_id, turn_ord, is_sidechain_turn, preview, user_prompt_text, \
              start_timestamp, end_timestamp, duration_ms, event_count, operation_count, \
              thinking_count, has_errors, markdown, turn_json, chunks_json) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
    )
    .bind(session_uuid)
    .bind(turn.turn.id)
    .bind(turn.turn_ord)
    .bind(turn.is_sidechain_turn)
    .bind(&turn.turn.preview)
    .bind(turn.turn.user_prompt_text.as_deref())
    .bind(turn.turn.start_timestamp)
    .bind(turn.turn.end_timestamp)
    .bind(turn.turn.duration_ms)
    .bind(turn.turn.event_count as i32)
    .bind(turn.turn.operation_count as i32)
    .bind(turn.turn.thinking_count as i32)
    .bind(turn.turn.has_errors)
    .bind(&turn.turn.markdown)
    .bind(serde_json::to_value(&turn.turn).context("serialize projected turn")?)
    .bind(serde_json::to_value(&turn.turn.chunks).context("serialize projected chunks")?)
    .execute(&mut **tx)
    .await
    .context("insert timeline_turns row")?;

    insert_projected_operations(tx, session_uuid, turn, expected_source_keys).await?;
    insert_projected_file_touches(tx, session_uuid, turn).await?;
    insert_projected_activity_signals(tx, session_uuid, turn).await?;
    Ok(())
}

async fn insert_projected_operations(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    turn: &StoredTurnProjection,
    expected_source_keys: &mut Vec<String>,
) -> anyhow::Result<()> {
    for operation in &turn.operations {
        sqlx::query(
            "INSERT INTO timeline_operations \
                 (session_uuid, turn_id, operation_ord, pair_id, name, raw_name, operation_type, \
                  operation_category, input, result_content, result_payload, result_is_error, \
                  is_error, is_pending, subagent_json) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
        )
        .bind(session_uuid)
        .bind(turn.turn.id)
        .bind(operation.operation_ord)
        .bind(&operation.pair_id)
        .bind(&operation.name)
        .bind(operation.raw_name.as_deref())
        .bind(operation.operation_type.as_deref())
        .bind(
            operation
                .operation_category
                .map(|category| category.as_str()),
        )
        .bind(operation.input.as_ref())
        .bind(operation.result_content.as_deref())
        .bind(operation.result_payload.as_ref())
        .bind(operation.result_is_error)
        .bind(operation.is_error)
        .bind(operation.is_pending)
        .bind(
            operation
                .subagent
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .context("serialize projected subagent")?,
        )
        .execute(&mut **tx)
        .await
        .context("insert timeline_operations row")?;
        enqueue_operation_embedding_sources(
            tx,
            session_uuid,
            turn.turn.id,
            operation,
            expected_source_keys,
        )
        .await?;
    }
    Ok(())
}

async fn enqueue_operation_embedding_sources(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    turn_id: i64,
    operation: &StoredOperationProjection,
    expected_source_keys: &mut Vec<String>,
) -> anyhow::Result<()> {
    let call_key = format!(
        "operation:{session_uuid}:{turn_id}:{}:call",
        operation.operation_ord
    );
    if let Some(text) = operation_call_text(operation).filter(|text| !text.trim().is_empty()) {
        upsert_operation_embedding_source(
            tx,
            "operation_call",
            "tool_call",
            &call_key,
            session_uuid,
            turn_id,
            operation.operation_ord,
            &text,
        )
        .await?;
        expected_source_keys.push(call_key);
    }

    let result_key = format!(
        "operation:{session_uuid}:{turn_id}:{}:result",
        operation.operation_ord
    );
    if let Some((source_kind, text)) =
        operation_result_text(operation).filter(|(_, text)| !text.trim().is_empty())
    {
        upsert_operation_embedding_source(
            tx,
            "operation_result",
            source_kind,
            &result_key,
            session_uuid,
            turn_id,
            operation.operation_ord,
            &text,
        )
        .await?;
        expected_source_keys.push(result_key);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn upsert_operation_embedding_source(
    tx: &mut Transaction<'_, Postgres>,
    source_family: &str,
    source_kind: &str,
    source_key: &str,
    session_uuid: Uuid,
    turn_id: i64,
    operation_ord: i32,
    text: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "WITH changed_source AS ( \
             INSERT INTO retrieval_embedding_sources \
            (source_family, source_kind, source_key, session_uuid, turn_id, operation_ord, \
             content_hash, index_status, index_error, last_seen_at, dirty_at, deleted_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending', NULL, NOW(), NOW(), NULL, NOW()) \
             ON CONFLICT (source_key) DO UPDATE SET \
                 source_family = EXCLUDED.source_family, \
                 source_kind = EXCLUDED.source_kind, \
                 session_uuid = EXCLUDED.session_uuid, \
                 byte_offset = NULL, \
                 block_ord = NULL, \
                 turn_id = EXCLUDED.turn_id, \
                 operation_ord = EXCLUDED.operation_ord, \
                 content_hash = EXCLUDED.content_hash, \
                 index_status = 'pending', \
                 index_error = NULL, \
                 last_seen_at = NOW(), \
                 dirty_at = NOW(), \
                 deleted_at = NULL, \
                 updated_at = NOW() \
             WHERE retrieval_embedding_sources.source_family IS DISTINCT FROM EXCLUDED.source_family \
                OR retrieval_embedding_sources.source_kind IS DISTINCT FROM EXCLUDED.source_kind \
                OR retrieval_embedding_sources.session_uuid IS DISTINCT FROM EXCLUDED.session_uuid \
                OR retrieval_embedding_sources.byte_offset IS NOT NULL \
                OR retrieval_embedding_sources.block_ord IS NOT NULL \
                OR retrieval_embedding_sources.turn_id IS DISTINCT FROM EXCLUDED.turn_id \
                OR retrieval_embedding_sources.operation_ord IS DISTINCT FROM EXCLUDED.operation_ord \
                OR retrieval_embedding_sources.content_hash IS DISTINCT FROM EXCLUDED.content_hash \
                OR retrieval_embedding_sources.index_status = 'deleted' \
                OR retrieval_embedding_sources.deleted_at IS NOT NULL \
             RETURNING source_key \
         ) \
         DELETE FROM retrieval_embeddings re \
          USING changed_source changed \
          WHERE re.source_key = changed.source_key",
    )
    .bind(source_family)
    .bind(source_kind)
    .bind(source_key)
    .bind(session_uuid)
    .bind(turn_id)
    .bind(operation_ord)
    .bind(hash_text(text.trim()))
    .execute(&mut **tx)
    .await
    .context("upsert operation retrieval source")?;
    Ok(())
}

fn operation_call_text(operation: &StoredOperationProjection) -> Option<String> {
    let input = operation.input.as_ref()?;
    let mut parts = vec![operation.name.clone()];
    if let Some(raw_name) = &operation.raw_name {
        parts.push(raw_name.clone());
    }
    if let Some(operation_type) = &operation.operation_type {
        parts.push(operation_type.clone());
    }
    if let Some(category) = operation.operation_category {
        parts.push(category.as_str().to_string());
    }
    parts.push(input.to_string());
    Some(parts.join(" "))
}

fn operation_result_text(operation: &StoredOperationProjection) -> Option<(&'static str, String)> {
    if operation.result_content.is_none()
        && operation.result_payload.is_none()
        && !operation.result_is_error
        && !operation.is_error
    {
        return None;
    }
    let payload = operation.result_payload.as_ref().map(ToString::to_string);
    let text = [
        Some(operation.name.as_str()),
        operation.result_content.as_deref(),
        payload.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    let source_kind = if operation.result_is_error || operation.is_error {
        "tool_error"
    } else {
        "tool_result"
    };
    Some((source_kind, text))
}

fn hash_text(text: &str) -> String {
    let hash = digest::digest(&digest::SHA256, text.as_bytes());
    hash.as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn insert_projected_file_touches(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    turn: &StoredTurnProjection,
) -> anyhow::Result<()> {
    for touch in &turn.file_touches {
        sqlx::query(
            "INSERT INTO timeline_file_touches \
                 (session_uuid, turn_id, touch_ord, operation_ord, repo_name, repo_rel_path, touch_kind, is_write) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(session_uuid)
        .bind(turn.turn.id)
        .bind(touch.touch_ord)
        .bind(touch.operation_ord)
        .bind(&touch.repo_name)
        .bind(&touch.repo_rel_path)
        .bind(&touch.touch_kind)
        .bind(touch.is_write)
        .execute(&mut **tx)
        .await
        .context("insert timeline_file_touches row")?;
    }
    Ok(())
}

async fn insert_projected_activity_signals(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    turn: &StoredTurnProjection,
) -> anyhow::Result<()> {
    for signal in &turn.activity_signals {
        sqlx::query(
            "INSERT INTO timeline_activity_signals \
                 (session_uuid, turn_id, signal_ord, signal_type, signal_value, signal_count) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(session_uuid)
        .bind(turn.turn.id)
        .bind(signal.signal_ord)
        .bind(&signal.signal_type)
        .bind(signal.signal_value.as_deref())
        .bind(signal.signal_count)
        .execute(&mut **tx)
        .await
        .context("insert timeline_activity_signals row")?;
    }
    Ok(())
}
