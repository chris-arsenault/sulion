use anyhow::Context;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::db::Pool;

use super::{load_file_touch_context, load_projection_source_events};
use crate::ingest::timeline::{
    build_session_projection, load_session_events, SessionEventFilter, StoredEvent,
    StoredTurnProjection,
};

pub async fn rebuild_session_projection(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<usize> {
    let events = load_projection_source_events(pool, session_uuid)
        .await
        .context("load canonical projection events")?;
    let file_context = load_file_touch_context(pool, session_uuid).await?;
    let projected = build_session_projection(&events, file_context.as_ref());

    let mut tx = pool.begin().await.context("begin projection tx")?;
    clear_session_projection(&mut tx, session_uuid).await?;
    insert_projection_rows(&mut tx, session_uuid, &projected).await?;
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
    insert_projection_rows(&mut tx, session_uuid, &projected).await?;
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
) -> anyhow::Result<()> {
    for turn in projected {
        insert_projected_turn(tx, session_uuid, turn).await?;
    }
    Ok(())
}

async fn insert_projected_turn(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    turn: &StoredTurnProjection,
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

    insert_projected_operations(tx, session_uuid, turn).await?;
    insert_projected_file_touches(tx, session_uuid, turn).await?;
    insert_projected_activity_signals(tx, session_uuid, turn).await?;
    Ok(())
}

async fn insert_projected_operations(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    turn: &StoredTurnProjection,
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
    }
    Ok(())
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
