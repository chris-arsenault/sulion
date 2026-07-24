//! Nonblocking startup recompute of historical token accounting.
//!
//! Claude Code transcripts repeat the same `message.usage` on every
//! content-block line of an API response, and rows written before the
//! message-id dedup landed are inflated by roughly the duplication factor.
//! This task recomputes every Claude session's totals from the stored raw
//! events — counting each `message.id` once and splitting cache reads out
//! of fresh input — and seeds `agent_usage_daily` history for both agents.
//!
//! It runs as a spawned task after boot (never inside a migration), works
//! in small session batches with pauses between them, and records a marker
//! in `usage_backfills` so it executes once per deploy generation.

use std::time::Duration;

use uuid::Uuid;

use crate::db::Pool;

const MARKER_ID: &str = "claude-usage-dedup-v1";
const BATCH_SIZE: i64 = 50;
const BATCH_PAUSE: Duration = Duration::from_millis(250);

pub async fn run_usage_backfill(pool: Pool) {
    match run(&pool).await {
        Ok(true) => tracing::info!("usage backfill completed"),
        Ok(false) => {}
        Err(err) => tracing::error!(%err, "usage backfill failed; will retry next boot"),
    }
}

async fn run(pool: &Pool) -> anyhow::Result<bool> {
    let done: Option<(String,)> = sqlx::query_as("SELECT id FROM usage_backfills WHERE id = $1")
        .bind(MARKER_ID)
        .fetch_optional(pool)
        .await?;
    if done.is_some() {
        return Ok(false);
    }

    let sessions: Vec<(Uuid,)> =
        sqlx::query_as("SELECT session_uuid FROM agent_session_usage ORDER BY session_uuid")
            .fetch_all(pool)
            .await?;
    let total = sessions.len();
    tracing::info!(total, "usage backfill starting");

    for (index, chunk) in sessions.chunks(BATCH_SIZE as usize).enumerate() {
        let ids: Vec<Uuid> = chunk.iter().map(|(id,)| *id).collect();
        recompute_claude_sessions(pool, &ids).await?;
        seed_daily_history(pool, &ids).await?;
        if index % 10 == 9 {
            tracing::info!(
                done = (index + 1) * BATCH_SIZE as usize,
                total,
                "usage backfill progress"
            );
        }
        tokio::time::sleep(BATCH_PAUSE).await;
    }

    sqlx::query("INSERT INTO usage_backfills (id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(MARKER_ID)
        .execute(pool)
        .await?;
    Ok(true)
}

/// Recompute Claude session totals from raw events, one count per message
/// id. The update is guarded on byte offset so a session that ingested new
/// events between our read and write is left for live ingest to extend.
async fn recompute_claude_sessions(pool: &Pool, ids: &[Uuid]) -> anyhow::Result<()> {
    sqlx::query(CLAUDE_RECOMPUTE_SQL)
        .bind(ids)
        .execute(pool)
        .await?;
    Ok(())
}

const CLAUDE_RECOMPUTE_SQL: &str = "\
WITH lines AS (
    SELECT e.session_uuid,
           COALESCE(e.payload #>> '{message,id}', e.byte_offset::TEXT) AS message_id,
           e.byte_offset,
           e.timestamp,
           e.payload #> '{message,usage}' AS usage
    FROM events e
    WHERE e.session_uuid = ANY($1)
      AND e.agent = 'claude-code'
      AND e.payload ->> 'type' = 'assistant'
      AND jsonb_typeof(e.payload #> '{message,usage}') = 'object'
),
messages AS (
    SELECT DISTINCT ON (session_uuid, message_id)
        session_uuid,
        message_id,
        byte_offset,
        timestamp,
        GREATEST(COALESCE((usage ->> 'input_tokens')::NUMERIC::BIGINT, 0), 0)
            + GREATEST(COALESCE((usage ->> 'cache_creation_input_tokens')::NUMERIC::BIGINT, 0), 0)
            AS input_tokens,
        GREATEST(COALESCE((usage ->> 'cache_read_input_tokens')::NUMERIC::BIGINT, 0), 0)
            AS cached_input_tokens,
        GREATEST(COALESCE((usage ->> 'output_tokens')::NUMERIC::BIGINT, 0), 0) AS output_tokens
    FROM lines
    ORDER BY session_uuid, message_id, byte_offset
),
sums AS (
    SELECT session_uuid,
           SUM(input_tokens) AS input_tokens,
           SUM(cached_input_tokens) AS cached_input_tokens,
           SUM(output_tokens) AS output_tokens,
           MAX(byte_offset) AS last_byte_offset,
           MAX(timestamp) AS observed_at
    FROM messages
    GROUP BY session_uuid
),
last_message AS (
    SELECT DISTINCT ON (session_uuid)
        session_uuid,
        message_id,
        input_tokens + cached_input_tokens + output_tokens AS context_tokens
    FROM messages
    ORDER BY session_uuid, byte_offset DESC
)
UPDATE agent_session_usage u SET
    input_tokens = s.input_tokens,
    cached_input_tokens = s.cached_input_tokens,
    output_tokens = s.output_tokens,
    reasoning_output_tokens = 0,
    total_tokens = s.input_tokens + s.cached_input_tokens + s.output_tokens,
    context_tokens = lm.context_tokens,
    last_byte_offset = s.last_byte_offset,
    observed_at = s.observed_at,
    last_usage_message_id = lm.message_id,
    updated_at = NOW()
FROM sums s
JOIN last_message lm USING (session_uuid)
WHERE u.session_uuid = s.session_uuid
  AND u.agent = 'claude-code'
  AND u.last_byte_offset <= s.last_byte_offset";

/// Seed `agent_usage_daily` history from raw events so tokens-per-day
/// charts have a past. Claude days sum deduped per-message usage
/// cumulatively; Codex days take the last cumulative snapshot of each day.
async fn seed_daily_history(pool: &Pool, ids: &[Uuid]) -> anyhow::Result<()> {
    sqlx::query(CLAUDE_DAILY_SQL)
        .bind(ids)
        .execute(pool)
        .await?;
    sqlx::query(CODEX_DAILY_SQL).bind(ids).execute(pool).await?;
    Ok(())
}

const CLAUDE_DAILY_SQL: &str = "\
WITH lines AS (
    SELECT e.session_uuid,
           COALESCE(e.payload #>> '{message,id}', e.byte_offset::TEXT) AS message_id,
           e.byte_offset,
           e.timestamp,
           e.payload #> '{message,usage}' AS usage
    FROM events e
    WHERE e.session_uuid = ANY($1)
      AND e.agent = 'claude-code'
      AND e.payload ->> 'type' = 'assistant'
      AND jsonb_typeof(e.payload #> '{message,usage}') = 'object'
),
messages AS (
    SELECT DISTINCT ON (session_uuid, message_id)
        session_uuid,
        timestamp::DATE AS day,
        GREATEST(COALESCE((usage ->> 'input_tokens')::NUMERIC::BIGINT, 0), 0)
            + GREATEST(COALESCE((usage ->> 'cache_creation_input_tokens')::NUMERIC::BIGINT, 0), 0)
            AS input_tokens,
        GREATEST(COALESCE((usage ->> 'cache_read_input_tokens')::NUMERIC::BIGINT, 0), 0)
            AS cached_input_tokens,
        GREATEST(COALESCE((usage ->> 'output_tokens')::NUMERIC::BIGINT, 0), 0) AS output_tokens
    FROM lines
    ORDER BY session_uuid, message_id, byte_offset
),
per_day AS (
    SELECT session_uuid, day,
           SUM(input_tokens) AS input_tokens,
           SUM(cached_input_tokens) AS cached_input_tokens,
           SUM(output_tokens) AS output_tokens
    FROM messages
    GROUP BY session_uuid, day
),
cumulative AS (
    SELECT session_uuid, day,
           SUM(input_tokens) OVER w AS input_tokens,
           SUM(cached_input_tokens) OVER w AS cached_input_tokens,
           SUM(output_tokens) OVER w AS output_tokens
    FROM per_day
    WINDOW w AS (PARTITION BY session_uuid ORDER BY day)
)
INSERT INTO agent_usage_daily
    (day, session_uuid, agent, input_tokens, cached_input_tokens, output_tokens,
     reasoning_output_tokens, total_tokens)
SELECT day, session_uuid, 'claude-code', input_tokens, cached_input_tokens, output_tokens,
       0, input_tokens + cached_input_tokens + output_tokens
FROM cumulative
ON CONFLICT (day, session_uuid) DO UPDATE SET
    agent = EXCLUDED.agent,
    input_tokens = EXCLUDED.input_tokens,
    cached_input_tokens = EXCLUDED.cached_input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    reasoning_output_tokens = EXCLUDED.reasoning_output_tokens,
    total_tokens = EXCLUDED.total_tokens,
    updated_at = NOW()";

const CODEX_DAILY_SQL: &str = "\
WITH snapshots AS (
    SELECT DISTINCT ON (e.session_uuid, e.timestamp::DATE)
        e.session_uuid,
        e.timestamp::DATE AS day,
        e.payload #> '{payload,info,total_token_usage}' AS total
    FROM events e
    WHERE e.session_uuid = ANY($1)
      AND e.agent = 'codex'
      AND e.payload #>> '{payload,type}' = 'token_count'
      AND jsonb_typeof(e.payload #> '{payload,info,total_token_usage}') = 'object'
    ORDER BY e.session_uuid, e.timestamp::DATE, e.byte_offset DESC
)
INSERT INTO agent_usage_daily
    (day, session_uuid, agent, input_tokens, cached_input_tokens, output_tokens,
     reasoning_output_tokens, total_tokens)
SELECT day, session_uuid, 'codex',
    GREATEST(COALESCE((total ->> 'input_tokens')::NUMERIC::BIGINT, 0), 0),
    GREATEST(COALESCE((total ->> 'cached_input_tokens')::NUMERIC::BIGINT, 0), 0),
    GREATEST(COALESCE((total ->> 'output_tokens')::NUMERIC::BIGINT, 0), 0),
    GREATEST(COALESCE((total ->> 'reasoning_output_tokens')::NUMERIC::BIGINT, 0), 0),
    GREATEST(
        COALESCE(
            (total ->> 'total_tokens')::NUMERIC::BIGINT,
            COALESCE((total ->> 'input_tokens')::NUMERIC::BIGINT, 0)
                + COALESCE((total ->> 'output_tokens')::NUMERIC::BIGINT, 0)
        ),
        0
    )
FROM snapshots
ON CONFLICT (day, session_uuid) DO UPDATE SET
    agent = EXCLUDED.agent,
    input_tokens = EXCLUDED.input_tokens,
    cached_input_tokens = EXCLUDED.cached_input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    reasoning_output_tokens = EXCLUDED.reasoning_output_tokens,
    total_tokens = EXCLUDED.total_tokens,
    updated_at = NOW()";
