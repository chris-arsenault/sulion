//! The single timeline writer. Each batch locks the session's state row,
//! feeds the next events after its cursor through the reducer, and commits
//! what they changed with the advanced cursor. A rebuild is the same loop
//! from an emptied projection.

use anyhow::Context;
use chrono::{DateTime, Utc};
use ring::digest;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::db::Pool;
use crate::ingest::timeline::reduce::{Changes, OpRow, Reducer, SessionState, TurnRow};
use crate::ingest::timeline::{load_session_events, SessionEventFilter, TimelineItem};

use super::load_file_touch_context;
use super::store::PgBackend;

/// Reducer rules the stored rows were built with. A session below it is
/// rebuilt from its canonical events before it takes new ones.
pub const REDUCER_VERSION: i32 = 1;

/// Events applied per transaction.
pub const BATCH_EVENTS: i64 = 500;

#[derive(Debug, Default, Clone, Copy)]
pub struct BatchOutcome {
    pub events: usize,
    /// More events were waiting when the batch closed.
    pub more: bool,
}

/// A purged session's `timeline_turns` rows are its digest, the only record
/// it has left; its `events` are gone. Every write entry point checks this
/// first, because projecting from zero events would delete the digest.
async fn session_is_purged(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<bool> {
    let purged: Option<bool> = sqlx::query_scalar(
        "SELECT purged_at IS NOT NULL FROM claude_sessions WHERE session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_optional(pool)
    .await
    .context("check purged state before projection")?;
    Ok(purged.unwrap_or(false))
}

#[derive(FromRow)]
struct StateRow {
    revision: i64,
    projected_through: i64,
    projection_version: i32,
    next_turn_ord: i32,
    current_main_turn_id: Option<i64>,
    current_sidechain_turn_id: Option<i64>,
    codex_input_total: Option<i64>,
    codex_output_total: Option<i64>,
    total_event_count: i64,
    turn_count: i64,
    latest_turn_id: Option<i64>,
    latest_event_at: Option<DateTime<Utc>>,
    reconcile_sources: bool,
}

impl StateRow {
    fn state(&self) -> SessionState {
        SessionState {
            projected_through: self.projected_through,
            next_turn_ord: self.next_turn_ord,
            current_main: self.current_main_turn_id,
            current_sidechain: self.current_sidechain_turn_id,
            codex_input_total: self.codex_input_total,
            codex_output_total: self.codex_output_total,
            total_event_count: self.total_event_count,
            turn_count: self.turn_count,
            latest_turn_id: self.latest_turn_id,
            latest_event_at: self.latest_event_at,
        }
    }

    fn cleared(revision: i64) -> Self {
        let state = SessionState::default();
        Self {
            revision,
            projected_through: state.projected_through,
            projection_version: REDUCER_VERSION,
            next_turn_ord: 0,
            current_main_turn_id: None,
            current_sidechain_turn_id: None,
            codex_input_total: None,
            codex_output_total: None,
            total_event_count: 0,
            turn_count: 0,
            latest_turn_id: None,
            latest_event_at: None,
            reconcile_sources: true,
        }
    }
}

/// Apply up to `budget` events after the session's cursor.
pub async fn project_batch(
    pool: &Pool,
    session_uuid: Uuid,
    budget: i64,
) -> anyhow::Result<BatchOutcome> {
    if session_is_purged(pool, session_uuid).await? {
        return Ok(BatchOutcome::default());
    }
    let mut tx = pool.begin().await.context("begin projection batch")?;
    sqlx::query(
        "INSERT INTO timeline_session_state (session_uuid) VALUES ($1) ON CONFLICT DO NOTHING",
    )
    .bind(session_uuid)
    .execute(&mut *tx)
    .await
    .context("ensure timeline session state")?;
    let mut row: StateRow = sqlx::query_as(
        "SELECT revision, projected_through, projection_version, next_turn_ord, \
                current_main_turn_id, current_sidechain_turn_id, codex_input_total, \
                codex_output_total, total_event_count, turn_count, latest_turn_id, \
                latest_event_at, reconcile_sources \
           FROM timeline_session_state WHERE session_uuid = $1 FOR UPDATE",
    )
    .bind(session_uuid)
    .fetch_one(&mut *tx)
    .await
    .context("lock timeline session state")?;

    if row.projection_version != REDUCER_VERSION {
        for table in ["timeline_turns", "timeline_message_usage"] {
            // Items, operations and file touches cascade from their turn.
            sqlx::query(&format!("DELETE FROM {table} WHERE session_uuid = $1"))
                .bind(session_uuid)
                .execute(&mut *tx)
                .await
                .with_context(|| format!("clear {table}"))?;
        }
        row = StateRow::cleared(row.revision + 1);
        write_state(&mut tx, session_uuid, &row.state(), &row).await?;
    }

    let events = load_session_events(
        pool,
        session_uuid,
        &SessionEventFilter {
            after: Some(row.projected_through),
            limit: Some(budget),
            kind: None,
        },
    )
    .await
    .context("load projection batch events")?;
    let applied = events.len();
    let more = applied as i64 >= budget;

    if applied > 0 {
        let file_context = load_file_touch_context(pool, session_uuid).await?;
        let child_of = claude_subagent_parent(&mut tx, session_uuid).await?;
        let backend = PgBackend { tx, session_uuid };
        let mut reducer = Reducer::new(backend, session_uuid, row.state(), file_context, child_of);
        for event in events {
            reducer.apply(event).await?;
        }
        let (backend, changes) = reducer.into_changes();
        tx = backend.tx;
        if !changes.is_empty() {
            row.revision += 1;
        }
        persist(&mut tx, session_uuid, &changes).await?;
        write_state(&mut tx, session_uuid, &changes.state, &row).await?;
    }

    if !more && row.reconcile_sources {
        reconcile_operation_sources(&mut tx, session_uuid).await?;
        sqlx::query(
            "UPDATE timeline_session_state SET reconcile_sources = FALSE WHERE session_uuid = $1",
        )
        .bind(session_uuid)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await.context("commit projection batch")?;
    Ok(BatchOutcome {
        events: applied,
        more,
    })
}

/// Apply every waiting event.
pub async fn project_until_caught_up(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<usize> {
    let mut total = 0;
    loop {
        let outcome = project_batch(pool, session_uuid, BATCH_EVENTS).await?;
        total += outcome.events;
        if !outcome.more {
            return Ok(total);
        }
    }
}

/// Ask for the session to be rebuilt from its canonical events on its next
/// batch. Used when events were inserted behind the cursor.
pub async fn request_rebuild(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<()> {
    sqlx::query("UPDATE timeline_session_state SET projection_version = 0 WHERE session_uuid = $1")
        .bind(session_uuid)
        .execute(pool)
        .await
        .context("request timeline rebuild")?;
    Ok(())
}

/// Rebuild the session from its canonical events. Returns its turn count.
pub async fn rebuild_session_projection(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<usize> {
    if session_is_purged(pool, session_uuid).await? {
        return Ok(0);
    }
    request_rebuild(pool, session_uuid).await?;
    project_until_caught_up(pool, session_uuid).await?;
    let turns: Option<i64> =
        sqlx::query_scalar("SELECT turn_count FROM timeline_session_state WHERE session_uuid = $1")
            .bind(session_uuid)
            .fetch_optional(pool)
            .await?;
    Ok(turns.unwrap_or(0) as usize)
}

/// Rebuild every retained session from its canonical events, most recently
/// active first. Each is marked up front, so the live ingester rebuilds a
/// session that receives events before this pass reaches it, and this pass
/// then only catches it up.
pub async fn backfill_timeline_projection(
    pool: &Pool,
    job: Option<&crate::ingest::jobs::JobHandle>,
) -> anyhow::Result<usize> {
    sqlx::query(
        "UPDATE timeline_session_state s SET projection_version = 0 \
           FROM claude_sessions cs \
          WHERE cs.session_uuid = s.session_uuid AND cs.purged_at IS NULL",
    )
    .execute(pool)
    .await
    .context("mark sessions for timeline rebuild")?;
    let sessions: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT cs.session_uuid \
           FROM claude_sessions cs \
           LEFT JOIN timeline_session_state s ON s.session_uuid = cs.session_uuid \
          WHERE cs.purged_at IS NULL \
            AND COALESCE(s.projection_version, 0) <> $1 \
            AND EXISTS (SELECT 1 FROM events e WHERE e.session_uuid = cs.session_uuid) \
          ORDER BY s.latest_event_at DESC NULLS LAST, cs.started_at DESC, cs.session_uuid",
    )
    .bind(REDUCER_VERSION)
    .fetch_all(pool)
    .await
    .context("list sessions requiring timeline projection rebuild")?;

    if let Some(job) = job {
        job.set_total(sessions.len() as i64).await;
    }
    for (session_uuid,) in &sessions {
        project_until_caught_up(pool, *session_uuid).await?;
        if let Some(job) = job {
            job.advance(Some(&session_uuid.to_string())).await;
        }
    }
    Ok(sessions.len())
}

/// Write each turn's digest into `timeline_turns.markdown`, composed from its
/// items and operations. The archive purge calls this before it deletes
/// them; live turns compose the digest on read instead.
pub async fn store_turn_digests(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
) -> anyhow::Result<u64> {
    let turns: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT turn_id, user_prompt_text FROM timeline_turns \
          WHERE session_uuid = $1 AND markdown = ''",
    )
    .bind(session_uuid)
    .fetch_all(&mut **tx)
    .await
    .context("load turns without a digest")?;
    let mut stored = 0;
    for (turn_id, user_prompt_text) in turns {
        let rows: Vec<(i64, serde_json::Value)> = sqlx::query_as(
            "SELECT byte_offset, body FROM timeline_items \
              WHERE session_uuid = $1 AND turn_id = $2 ORDER BY byte_offset",
        )
        .bind(session_uuid)
        .bind(turn_id)
        .fetch_all(&mut **tx)
        .await?;
        let items: Vec<TimelineItem> = rows
            .into_iter()
            .map(|(offset, body)| {
                Ok(TimelineItem {
                    offset,
                    chunk: serde_json::from_value(body)?,
                })
            })
            .collect::<anyhow::Result<_>>()
            .context("decode timeline items")?;
        let operations: Vec<super::ProjectedOperationRow> = sqlx::query_as(
            "SELECT turn_id, operation_ord, pair_id, name, raw_name, operation_type, \
                    operation_category, input, result_content, result_payload, result_is_error, \
                    is_error, is_pending \
               FROM timeline_operations \
              WHERE session_uuid = $1 AND turn_id = $2 ORDER BY operation_ord",
        )
        .bind(session_uuid)
        .bind(turn_id)
        .fetch_all(&mut **tx)
        .await?;
        let mut touches = std::collections::HashMap::new();
        let pairs: Vec<_> = operations
            .into_iter()
            .map(|row| super::build_operation_pair(row, &mut touches))
            .collect::<anyhow::Result<_>>()?;
        let markdown = crate::ingest::timeline::compose_turn_markdown(
            user_prompt_text.as_deref(),
            &items,
            &pairs,
        );
        stored += sqlx::query(
            "UPDATE timeline_turns SET markdown = $3 WHERE session_uuid = $1 AND turn_id = $2",
        )
        .bind(session_uuid)
        .bind(turn_id)
        .bind(markdown)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    }
    Ok(stored)
}

/// A Claude subagent transcript links its events to the spawning call in
/// its parent. Other child relations, such as a compaction continuation,
/// share `parent_session_uuid` but spawn nothing.
async fn claude_subagent_parent(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
) -> anyhow::Result<Option<Uuid>> {
    let row: Option<(Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT cs.parent_session_uuid, i.file_path \
           FROM claude_sessions cs \
           LEFT JOIN ingester_state i ON i.session_uuid = cs.session_uuid \
          WHERE cs.session_uuid = $1 AND cs.agent = 'claude-code'",
    )
    .bind(session_uuid)
    .fetch_optional(&mut **tx)
    .await
    .context("load subagent parent")?;
    Ok(row.and_then(|(parent, path)| {
        parent.filter(|_| path.is_some_and(|path| path.contains("/subagents/agent-")))
    }))
}

async fn write_state(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    state: &SessionState,
    row: &StateRow,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE timeline_session_state SET \
             revision = $2, projected_through = $3, projection_version = $4, next_turn_ord = $5, \
             current_main_turn_id = $6, current_sidechain_turn_id = $7, \
             codex_input_total = $8, codex_output_total = $9, total_event_count = $10, \
             turn_count = $11, latest_turn_id = $12, latest_event_at = $13, \
             reconcile_sources = $14, updated_at = NOW() \
         WHERE session_uuid = $1",
    )
    .bind(session_uuid)
    .bind(row.revision)
    .bind(state.projected_through)
    .bind(row.projection_version)
    .bind(state.next_turn_ord)
    .bind(state.current_main)
    .bind(state.current_sidechain)
    .bind(state.codex_input_total)
    .bind(state.codex_output_total)
    .bind(state.total_event_count)
    .bind(state.turn_count)
    .bind(state.latest_turn_id)
    .bind(state.latest_event_at)
    .bind(row.reconcile_sources)
    .execute(&mut **tx)
    .await
    .context("write timeline session state")?;
    Ok(())
}

// ── persistence ──────────────────────────────────────────────────────────

async fn persist(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    changes: &Changes,
) -> anyhow::Result<()> {
    for turn in &changes.turns {
        upsert_turn(tx, session_uuid, turn).await?;
    }
    for (turn_id, offset, chunk) in &changes.items {
        sqlx::query(
            "INSERT INTO timeline_items (session_uuid, turn_id, byte_offset, body) \
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(session_uuid)
        .bind(turn_id)
        .bind(offset)
        .bind(serde_json::to_value(chunk).context("serialize timeline item")?)
        .execute(&mut **tx)
        .await
        .context("insert timeline item")?;
    }
    for op in &changes.new_ops {
        insert_operation(tx, session_uuid, op).await?;
        refresh_operation_sources(tx, session_uuid, op).await?;
    }
    for (op, input_changed) in &changes.updated_ops {
        update_operation(tx, session_uuid, op, *input_changed).await?;
        refresh_operation_sources(tx, session_uuid, op).await?;
    }
    for (turn_id, operation_ord, touches) in &changes.touches {
        replace_file_touches(tx, session_uuid, *turn_id, *operation_ord, touches).await?;
    }
    for (message_id, usage) in &changes.usage {
        sqlx::query(
            "INSERT INTO timeline_message_usage \
                 (session_uuid, message_id, turn_id, input_tokens, output_tokens) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (session_uuid, message_id) DO UPDATE SET \
                 turn_id = EXCLUDED.turn_id, input_tokens = EXCLUDED.input_tokens, \
                 output_tokens = EXCLUDED.output_tokens",
        )
        .bind(session_uuid)
        .bind(message_id)
        .bind(usage.turn_id)
        .bind(usage.input_tokens)
        .bind(usage.output_tokens)
        .execute(&mut **tx)
        .await
        .context("record response usage")?;
    }
    for link in &changes.links {
        sqlx::query(
            "INSERT INTO timeline_child_links \
                 (session_uuid, pair_id, child_session_uuid, child_turn_id) \
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(link.session_uuid)
        .bind(&link.pair_id)
        .bind(link.child_session_uuid)
        .bind(link.child_turn_id)
        .execute(&mut **tx)
        .await
        .context("record child link")?;
    }
    Ok(())
}

async fn upsert_turn(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    turn: &TurnRow,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO timeline_turns \
             (session_uuid, turn_id, turn_ord, is_sidechain_turn, preview, user_prompt_text, \
              prompt_event_uuid, start_timestamp, end_timestamp, duration_ms, event_count, \
              operation_count, thinking_count, has_errors, markdown, input_tokens, output_tokens, \
              usage_baseline_input, usage_baseline_output) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, '', $15, $16, \
                 $17, $18) \
         ON CONFLICT (session_uuid, turn_id) DO UPDATE SET \
             preview = EXCLUDED.preview, \
             end_timestamp = EXCLUDED.end_timestamp, \
             duration_ms = EXCLUDED.duration_ms, \
             event_count = EXCLUDED.event_count, \
             operation_count = EXCLUDED.operation_count, \
             thinking_count = EXCLUDED.thinking_count, \
             has_errors = EXCLUDED.has_errors, \
             input_tokens = EXCLUDED.input_tokens, \
             output_tokens = EXCLUDED.output_tokens",
    )
    .bind(session_uuid)
    .bind(turn.turn_id)
    .bind(turn.turn_ord)
    .bind(turn.is_sidechain)
    .bind(&turn.preview)
    .bind(turn.user_prompt_text.as_deref())
    .bind(turn.prompt_event_uuid.as_deref())
    .bind(turn.start_timestamp)
    .bind(turn.end_timestamp)
    .bind(turn.duration_ms)
    .bind(turn.event_count)
    .bind(turn.operation_count)
    .bind(turn.thinking_count)
    .bind(turn.has_errors)
    .bind(turn.input_tokens)
    .bind(turn.output_tokens)
    .bind(turn.usage_baseline_input)
    .bind(turn.usage_baseline_output)
    .execute(&mut **tx)
    .await
    .context("upsert timeline turn")?;
    Ok(())
}

async fn insert_operation(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    op: &OpRow,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO timeline_operations \
             (session_uuid, turn_id, operation_ord, pair_id, name, raw_name, operation_type, \
              operation_category, input, result_content, result_payload, result_is_error, \
              is_error, is_pending, call_offset, call_at, changed_at, call_error, running_cell, \
              finished_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, \
                 $18, $19, $20)",
    )
    .bind(session_uuid)
    .bind(op.turn_id)
    .bind(op.operation_ord)
    .bind(&op.pair_id)
    .bind(&op.name)
    .bind(op.raw_name.as_deref())
    .bind(op.operation_type.as_deref())
    .bind(op.category.map(|category| category.as_str()))
    .bind(op.input.as_ref())
    .bind(op.result_content.as_deref())
    .bind(op.result_payload.as_ref())
    .bind(op.result_is_error)
    .bind(op.is_error)
    .bind(op.is_pending)
    .bind(op.call_offset)
    .bind(op.call_at)
    .bind(op.changed_at)
    .bind(op.call_error)
    .bind(op.running_cell.as_deref())
    .bind(op.finished_at)
    .execute(&mut **tx)
    .await
    .context("insert timeline operation")?;
    Ok(())
}

/// A result, runtime evidence or completed wait changed the operation. The
/// call's input is written only when evidence changed it.
async fn update_operation(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    op: &OpRow,
    input_changed: bool,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE timeline_operations SET \
             input = CASE WHEN $4 THEN $5 ELSE input END, \
             result_content = $6, result_payload = $7, result_is_error = $8, is_error = $9, \
             is_pending = $10, changed_at = $11, call_error = $12, running_cell = $13, \
             finished_at = $14 \
         WHERE session_uuid = $1 AND turn_id = $2 AND operation_ord = $3",
    )
    .bind(session_uuid)
    .bind(op.turn_id)
    .bind(op.operation_ord)
    .bind(input_changed)
    .bind(op.input.as_ref())
    .bind(op.result_content.as_deref())
    .bind(op.result_payload.as_ref())
    .bind(op.result_is_error)
    .bind(op.is_error)
    .bind(op.is_pending)
    .bind(op.changed_at)
    .bind(op.call_error)
    .bind(op.running_cell.as_deref())
    .bind(op.finished_at)
    .execute(&mut **tx)
    .await
    .context("update timeline operation")?;
    Ok(())
}

/// Touches are numbered within their operation, so replacing one
/// operation's touches never renumbers another's.
async fn replace_file_touches(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    turn_id: i64,
    operation_ord: i32,
    touches: &[crate::ingest::timeline::TimelineFileTouch],
) -> anyhow::Result<()> {
    sqlx::query(
        "DELETE FROM timeline_file_touches \
          WHERE session_uuid = $1 AND turn_id = $2 AND operation_ord = $3",
    )
    .bind(session_uuid)
    .bind(turn_id)
    .bind(operation_ord)
    .execute(&mut **tx)
    .await
    .context("clear operation file touches")?;
    for (index, touch) in touches.iter().enumerate().take(TOUCHES_PER_OPERATION) {
        sqlx::query(
            "INSERT INTO timeline_file_touches \
                 (session_uuid, turn_id, touch_ord, operation_ord, repo_name, repo_rel_path, \
                  touch_kind, is_write) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(session_uuid)
        .bind(turn_id)
        .bind(operation_ord * TOUCHES_PER_OPERATION as i32 + index as i32)
        .bind(operation_ord)
        .bind(&touch.repo)
        .bind(&touch.path)
        .bind(&touch.touch_kind)
        .bind(touch.is_write)
        .execute(&mut **tx)
        .await
        .context("insert operation file touch")?;
    }
    Ok(())
}

const TOUCHES_PER_OPERATION: usize = 1024;

/// Refreshes the operation's retrieval sources from its stored row. The call
/// and result texts are formed in SQL so their hashes match earlier ones.
async fn refresh_operation_sources(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    op: &OpRow,
) -> anyhow::Result<()> {
    let (call_text, result_text): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT \
             CASE \
               WHEN input IS NOT NULL \
                AND length(trim(concat_ws(' ', name, raw_name, operation_type, operation_category, input::TEXT))) > 0 \
               THEN concat_ws(' ', name, raw_name, operation_type, operation_category, left(input::TEXT, 300)) \
             END, \
             CASE \
               WHEN (result_content IS NOT NULL OR result_payload IS NOT NULL OR result_is_error OR is_error) \
                AND length(trim(concat_ws(' ', name, result_content, result_payload::TEXT))) > 0 \
                AND (result_is_error OR is_error OR name = 'agent') \
               THEN CASE \
                      WHEN result_is_error OR is_error \
                      THEN left(concat_ws(' ', name, result_content, result_payload::TEXT), 1000) \
                      ELSE concat_ws(' ', name, result_content, result_payload::TEXT) \
                    END \
             END \
           FROM timeline_operations \
          WHERE session_uuid = $1 AND turn_id = $2 AND operation_ord = $3",
    )
    .bind(session_uuid)
    .bind(op.turn_id)
    .bind(op.operation_ord)
    .fetch_one(&mut **tx)
    .await
    .context("form operation retrieval text")?;

    let base = format!(
        "operation:{session_uuid}:{}:{}",
        op.turn_id, op.operation_ord
    );
    if let Some(text) = call_text {
        let key = format!("{base}:call");
        upsert_operation_source(tx, "operation_call", "tool_call", &key, op, &text).await?;
    }
    if let Some(text) = result_text {
        let kind = if op.result_is_error || op.is_error {
            "tool_error"
        } else {
            "tool_result"
        };
        let key = format!("{base}:result");
        upsert_operation_source(tx, "operation_result", kind, &key, op, &text).await?;
    }
    Ok(())
}

async fn upsert_operation_source(
    tx: &mut Transaction<'_, Postgres>,
    source_family: &str,
    source_kind: &str,
    source_key: &str,
    op: &OpRow,
    text: &str,
) -> anyhow::Result<()> {
    let session_uuid: Uuid = source_key
        .split(':')
        .nth(1)
        .and_then(|id| Uuid::parse_str(id).ok())
        .context("operation source key names its session")?;
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
    .bind(op.turn_id)
    .bind(op.operation_ord)
    .bind(hash_text(text))
    .execute(&mut **tx)
    .await
    .context("upsert operation retrieval source")?;
    Ok(())
}

/// After a rebuild, operation sources whose operation did not come back.
async fn reconcile_operation_sources(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
) -> anyhow::Result<()> {
    sqlx::query(
        "WITH stale AS ( \
             SELECT source_key FROM retrieval_embedding_sources s \
              WHERE s.session_uuid = $1 \
                AND s.source_family IN ('operation_call', 'operation_result') \
                AND s.deleted_at IS NULL \
                AND NOT EXISTS (SELECT 1 FROM timeline_operations o \
                                 WHERE o.session_uuid = s.session_uuid AND o.turn_id = s.turn_id \
                                   AND o.operation_ord = s.operation_ord) \
         ), removed_embeddings AS ( \
             DELETE FROM retrieval_embeddings re USING stale \
              WHERE re.source_key = stale.source_key RETURNING re.source_key \
         ) \
         UPDATE retrieval_embedding_sources s \
            SET index_status = 'deleted', deleted_at = NOW(), updated_at = NOW() \
           FROM stale WHERE s.source_key = stale.source_key",
    )
    .bind(session_uuid)
    .execute(&mut **tx)
    .await
    .context("reconcile operation retrieval sources")?;
    Ok(())
}

fn hash_text(text: &str) -> String {
    let hash = digest::digest(&digest::SHA256, text.as_bytes());
    hash.as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
