use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::ingester::TranscriptSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageMode {
    Cumulative,
    Delta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UsageUpdate {
    mode: UsageMode,
    /// API response id for Delta sources. Claude Code emits one JSONL line
    /// per content block and repeats the identical usage object on each, so
    /// deltas are counted once per message id, not once per line.
    message_id: Option<String>,
    model: Option<String>,
    /// Standard, non-cached input. Cache writes and reads stay separate so
    /// pricing never has to infer which provider rate applies.
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_input_tokens: i64,
    cache_write_1h_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
    context_tokens: Option<i64>,
    model_context_window: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct StoredUsage {
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_input_tokens: i64,
    cache_write_1h_input_tokens: i64,
    output_tokens: i64,
    last_usage_message_id: Option<String>,
}

pub(super) async fn upsert_from_event(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    source: TranscriptSource,
    byte_offset: i64,
    observed_at: DateTime<Utc>,
    value: &Value,
) -> Result<(), sqlx::Error> {
    let Some(usage) = extract_usage(source, value) else {
        return Ok(());
    };
    let previous: Option<StoredUsage> = sqlx::query_as(
        "SELECT input_tokens, cached_input_tokens, cache_write_input_tokens, \
                cache_write_1h_input_tokens, output_tokens, last_usage_message_id \
           FROM agent_session_usage WHERE session_uuid = $1 FOR UPDATE",
    )
    .bind(session_uuid)
    .fetch_optional(&mut **tx)
    .await?;
    let daily_delta = usage.daily_delta(previous.as_ref());
    let metadata_model: Option<(Option<String>,)> =
        sqlx::query_as("SELECT model FROM agent_session_metadata WHERE session_uuid = $1")
            .bind(session_uuid)
            .fetch_optional(&mut **tx)
            .await?;
    let model = usage
        .model
        .as_deref()
        .or_else(|| match source {
            TranscriptSource::Codex => metadata_model.as_ref().and_then(|row| row.0.as_deref()),
            TranscriptSource::ClaudeCode => None,
        })
        .unwrap_or("(unknown model)");
    let is_delta = usage.mode == UsageMode::Delta;
    sqlx::query(
        "INSERT INTO agent_session_usage \
            (session_uuid, agent, input_tokens, cached_input_tokens, \
             cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens, \
             reasoning_output_tokens, total_tokens, context_tokens, model_context_window, \
             last_byte_offset, observed_at, last_usage_message_id, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $15, NOW()) \
         ON CONFLICT (session_uuid) DO UPDATE SET \
            agent = EXCLUDED.agent, \
            input_tokens = CASE \
                WHEN NOT $14 THEN EXCLUDED.input_tokens \
                WHEN $15::TEXT IS NOT NULL \
                    AND $15 = agent_session_usage.last_usage_message_id \
                    THEN agent_session_usage.input_tokens \
                ELSE agent_session_usage.input_tokens + EXCLUDED.input_tokens END, \
            cached_input_tokens = CASE \
                WHEN NOT $14 THEN EXCLUDED.cached_input_tokens \
                WHEN $15::TEXT IS NOT NULL \
                    AND $15 = agent_session_usage.last_usage_message_id \
                    THEN agent_session_usage.cached_input_tokens \
                ELSE agent_session_usage.cached_input_tokens + EXCLUDED.cached_input_tokens END, \
            cache_write_input_tokens = CASE \
                WHEN NOT $14 THEN EXCLUDED.cache_write_input_tokens \
                WHEN $15::TEXT IS NOT NULL \
                    AND $15 = agent_session_usage.last_usage_message_id \
                    THEN agent_session_usage.cache_write_input_tokens \
                ELSE agent_session_usage.cache_write_input_tokens \
                    + EXCLUDED.cache_write_input_tokens END, \
            cache_write_1h_input_tokens = CASE \
                WHEN NOT $14 THEN EXCLUDED.cache_write_1h_input_tokens \
                WHEN $15::TEXT IS NOT NULL \
                    AND $15 = agent_session_usage.last_usage_message_id \
                    THEN agent_session_usage.cache_write_1h_input_tokens \
                ELSE agent_session_usage.cache_write_1h_input_tokens \
                    + EXCLUDED.cache_write_1h_input_tokens END, \
            output_tokens = CASE \
                WHEN NOT $14 THEN EXCLUDED.output_tokens \
                WHEN $15::TEXT IS NOT NULL \
                    AND $15 = agent_session_usage.last_usage_message_id \
                    THEN agent_session_usage.output_tokens \
                ELSE agent_session_usage.output_tokens + EXCLUDED.output_tokens END, \
            reasoning_output_tokens = CASE \
                WHEN NOT $14 THEN EXCLUDED.reasoning_output_tokens \
                WHEN $15::TEXT IS NOT NULL \
                    AND $15 = agent_session_usage.last_usage_message_id \
                    THEN agent_session_usage.reasoning_output_tokens \
                ELSE agent_session_usage.reasoning_output_tokens \
                    + EXCLUDED.reasoning_output_tokens END, \
            total_tokens = CASE \
                WHEN NOT $14 THEN EXCLUDED.total_tokens \
                WHEN $15::TEXT IS NOT NULL \
                    AND $15 = agent_session_usage.last_usage_message_id \
                    THEN agent_session_usage.total_tokens \
                ELSE agent_session_usage.total_tokens + EXCLUDED.total_tokens END, \
            context_tokens = COALESCE(EXCLUDED.context_tokens, agent_session_usage.context_tokens), \
            model_context_window = COALESCE( \
                EXCLUDED.model_context_window, agent_session_usage.model_context_window \
            ), \
            last_byte_offset = EXCLUDED.last_byte_offset, \
            observed_at = EXCLUDED.observed_at, \
            last_usage_message_id = COALESCE( \
                EXCLUDED.last_usage_message_id, agent_session_usage.last_usage_message_id \
            ), \
            updated_at = NOW() \
         WHERE EXCLUDED.last_byte_offset > agent_session_usage.last_byte_offset",
    )
    .bind(session_uuid)
    .bind(source.agent_id())
    .bind(usage.input_tokens)
    .bind(usage.cached_input_tokens)
    .bind(usage.cache_write_input_tokens)
    .bind(usage.cache_write_1h_input_tokens)
    .bind(usage.output_tokens)
    .bind(usage.reasoning_output_tokens)
    .bind(usage.total_tokens)
    .bind(usage.context_tokens)
    .bind(usage.model_context_window)
    .bind(byte_offset)
    .bind(observed_at)
    .bind(is_delta)
    .bind(usage.message_id.as_deref())
    .execute(&mut **tx)
    .await?;
    add_model_daily(
        tx,
        session_uuid,
        source.agent_id(),
        model,
        observed_at,
        daily_delta,
        usage.message_id.as_deref(),
    )
    .await?;
    snapshot_daily(tx, session_uuid, observed_at).await
}

impl UsageUpdate {
    fn daily_delta(&self, previous: Option<&StoredUsage>) -> UsageComponents {
        if self.mode == UsageMode::Delta {
            let duplicate = self.message_id.as_deref().is_some_and(|message_id| {
                previous.and_then(|row| row.last_usage_message_id.as_deref()) == Some(message_id)
            });
            if duplicate {
                return UsageComponents::default();
            }
            return UsageComponents {
                input_tokens: self.input_tokens,
                cached_input_tokens: self.cached_input_tokens,
                cache_write_input_tokens: self.cache_write_input_tokens,
                cache_write_1h_input_tokens: self.cache_write_1h_input_tokens,
                output_tokens: self.output_tokens,
            };
        }
        UsageComponents {
            input_tokens: self
                .input_tokens
                .saturating_sub(previous.map_or(0, |row| row.input_tokens)),
            cached_input_tokens: self
                .cached_input_tokens
                .saturating_sub(previous.map_or(0, |row| row.cached_input_tokens)),
            cache_write_input_tokens: self
                .cache_write_input_tokens
                .saturating_sub(previous.map_or(0, |row| row.cache_write_input_tokens)),
            cache_write_1h_input_tokens: self
                .cache_write_1h_input_tokens
                .saturating_sub(previous.map_or(0, |row| row.cache_write_1h_input_tokens)),
            output_tokens: self
                .output_tokens
                .saturating_sub(previous.map_or(0, |row| row.output_tokens)),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct UsageComponents {
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_input_tokens: i64,
    cache_write_1h_input_tokens: i64,
    output_tokens: i64,
}

async fn add_model_daily(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    agent: &str,
    model: &str,
    observed_at: DateTime<Utc>,
    usage: UsageComponents,
    message_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO agent_model_usage_daily ( \
            day, session_uuid, agent, model, input_tokens, cached_input_tokens, \
            cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens, \
            last_usage_message_id, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW()) \
         ON CONFLICT (day, session_uuid, model) DO UPDATE SET \
            agent = EXCLUDED.agent, \
            input_tokens = agent_model_usage_daily.input_tokens + EXCLUDED.input_tokens, \
            cached_input_tokens = agent_model_usage_daily.cached_input_tokens \
                + EXCLUDED.cached_input_tokens, \
            cache_write_input_tokens = agent_model_usage_daily.cache_write_input_tokens \
                + EXCLUDED.cache_write_input_tokens, \
            cache_write_1h_input_tokens = agent_model_usage_daily.cache_write_1h_input_tokens \
                + EXCLUDED.cache_write_1h_input_tokens, \
            output_tokens = agent_model_usage_daily.output_tokens + EXCLUDED.output_tokens, \
            last_usage_message_id = COALESCE( \
                EXCLUDED.last_usage_message_id, agent_model_usage_daily.last_usage_message_id), \
            updated_at = NOW()",
    )
    .bind(observed_at.date_naive())
    .bind(session_uuid)
    .bind(agent)
    .bind(model)
    .bind(usage.input_tokens)
    .bind(usage.cached_input_tokens)
    .bind(usage.cache_write_input_tokens)
    .bind(usage.cache_write_1h_input_tokens)
    .bind(usage.output_tokens)
    .bind(message_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// End-of-day cumulative snapshot retained for session-history consumers.
/// Metrics use the direct model/day deltas written by `add_model_daily`.
async fn snapshot_daily(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    observed_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO agent_usage_daily \
            (day, session_uuid, agent, input_tokens, cached_input_tokens, \
             cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens, \
             reasoning_output_tokens, total_tokens, updated_at) \
         SELECT $2::DATE, session_uuid, agent, input_tokens, cached_input_tokens, \
                cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens, \
                reasoning_output_tokens, total_tokens, NOW() \
         FROM agent_session_usage WHERE session_uuid = $1 \
         ON CONFLICT (day, session_uuid) DO UPDATE SET \
            agent = EXCLUDED.agent, \
            input_tokens = EXCLUDED.input_tokens, \
            cached_input_tokens = EXCLUDED.cached_input_tokens, \
            cache_write_input_tokens = EXCLUDED.cache_write_input_tokens, \
            cache_write_1h_input_tokens = EXCLUDED.cache_write_1h_input_tokens, \
            output_tokens = EXCLUDED.output_tokens, \
            reasoning_output_tokens = EXCLUDED.reasoning_output_tokens, \
            total_tokens = EXCLUDED.total_tokens, \
            updated_at = NOW()",
    )
    .bind(session_uuid)
    .bind(observed_at.date_naive())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn extract_usage(source: TranscriptSource, value: &Value) -> Option<UsageUpdate> {
    match source {
        TranscriptSource::Codex => extract_codex_usage(value),
        TranscriptSource::ClaudeCode => extract_claude_usage(value),
    }
}

fn extract_codex_usage(value: &Value) -> Option<UsageUpdate> {
    let payload = value.get("payload")?;
    if super::canonical::codex_record_kind(value) != Some("event_msg")
        || string_at(payload, &["type"]) != Some("token_count")
    {
        return None;
    }
    let info = payload.get("info")?;
    let total = info.get("total_token_usage")?;
    let last = info
        .get("last_token_usage")
        .filter(|usage| usage.is_object());
    let reasoning_output_tokens = token_at(total, "reasoning_output_tokens");
    let cached_input_tokens = token_at(total, "cached_input_tokens");
    let cache_write_input_tokens = token_at(total, "cache_write_input_tokens");
    let reported_input_tokens = token_at(total, "input_tokens");
    let total_tokens = total_token_count(total);
    let last_total = last.map_or(0, total_token_count);
    let last_reasoning = last.map_or(0, |usage| token_at(usage, "reasoning_output_tokens"));
    Some(UsageUpdate {
        mode: UsageMode::Cumulative,
        message_id: None,
        model: None,
        input_tokens: reported_input_tokens
            .saturating_sub(cached_input_tokens)
            .saturating_sub(cache_write_input_tokens),
        cached_input_tokens,
        cache_write_input_tokens,
        cache_write_1h_input_tokens: 0,
        output_tokens: token_at(total, "output_tokens"),
        reasoning_output_tokens,
        total_tokens,
        context_tokens: last.map(|_| last_total.saturating_sub(last_reasoning)),
        model_context_window: positive_i64_at(info, &["model_context_window"]),
    })
}

/// Claude reports standard input, cache writes, and cache reads as disjoint
/// categories. A top-level cache-creation total is retained for compatibility
/// with older transcript versions; any amount not marked as one-hour is the
/// ordinary/5-minute cache-write bucket.
fn extract_claude_usage(value: &Value) -> Option<UsageUpdate> {
    if string_at(value, &["type"]) != Some("assistant") {
        return None;
    }
    let message = value.get("message");
    let usage = message
        .and_then(|message| message.get("usage"))
        .or_else(|| value.get("usage"))?;
    if !usage.is_object() {
        return None;
    }
    let message_id = message
        .and_then(|message| message.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let model = message
        .and_then(|message| message.get("model"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let cache_creation = token_at(usage, "cache_creation_input_tokens");
    let cache_write_5m =
        positive_i64_at(usage, &["cache_creation", "ephemeral_5m_input_tokens"]).unwrap_or(0);
    let cache_write_1h =
        positive_i64_at(usage, &["cache_creation", "ephemeral_1h_input_tokens"]).unwrap_or(0);
    let cache_write = cache_creation
        .saturating_sub(cache_write_1h)
        .max(cache_write_5m);
    let cache_read = token_at(usage, "cache_read_input_tokens");
    let input_tokens = token_at(usage, "input_tokens");
    let output_tokens = token_at(usage, "output_tokens");
    let total_tokens = input_tokens
        .saturating_add(cache_write)
        .saturating_add(cache_write_1h)
        .saturating_add(cache_read)
        .saturating_add(output_tokens);
    Some(UsageUpdate {
        mode: UsageMode::Delta,
        message_id,
        model,
        input_tokens,
        cached_input_tokens: cache_read,
        cache_write_input_tokens: cache_write,
        cache_write_1h_input_tokens: cache_write_1h,
        output_tokens,
        reasoning_output_tokens: 0,
        total_tokens,
        context_tokens: Some(total_tokens),
        model_context_window: positive_i64_at(usage, &["model_context_window"])
            .or_else(|| positive_i64_at(value, &["model_context_window"])),
    })
}

fn string_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str()
}

fn positive_i64_at(value: &Value, path: &[&str]) -> Option<i64> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    let parsed = current
        .as_i64()
        .or_else(|| current.as_u64().and_then(|value| i64::try_from(value).ok()))?;
    (parsed > 0).then_some(parsed)
}

fn token_at(value: &Value, key: &str) -> i64 {
    positive_i64_at(value, &[key]).unwrap_or(0)
}

fn total_token_count(value: &Value) -> i64 {
    positive_i64_at(value, &["total_tokens"]).unwrap_or_else(|| {
        token_at(value, "input_tokens").saturating_add(token_at(value, "output_tokens"))
    })
}

/// Rebuild the derived usage tables from canonical event payloads. The usage
/// tables are locked before the event snapshot is read: an ingester transaction
/// that has inserted a newer event will then apply its usage update after this
/// transaction commits, so the rebuild cannot erase concurrent usage.
pub(super) async fn rebuild_usage_projection(pool: &crate::db::Pool) -> anyhow::Result<u64> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "LOCK TABLE agent_session_usage, agent_usage_daily, agent_model_usage_daily \
         IN ACCESS EXCLUSIVE MODE",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM agent_model_usage_daily")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM agent_usage_daily")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM agent_session_usage")
        .execute(&mut *tx)
        .await?;

    let codex_sessions = sqlx::query(
        "WITH raw AS ( \
            SELECT DISTINCT ON (e.session_uuid) \
                e.session_uuid, e.agent, e.byte_offset, e.timestamp, \
                e.payload #> '{payload,info,total_token_usage}' AS total_usage, \
                e.payload #> '{payload,info,last_token_usage}' AS last_usage, \
                e.payload #> '{payload,info,model_context_window}' AS context_window \
            FROM events e \
            WHERE e.agent = 'codex' \
              AND e.payload #>> '{payload,type}' = 'token_count' \
              AND jsonb_typeof(e.payload #> '{payload,info,total_token_usage}') = 'object' \
            ORDER BY e.session_uuid, e.byte_offset DESC \
         ), parsed AS ( \
            SELECT *, \
                CASE WHEN jsonb_typeof(total_usage -> 'input_tokens') = 'number' \
                    THEN (total_usage ->> 'input_tokens')::NUMERIC::BIGINT ELSE 0 END AS reported_input, \
                CASE WHEN jsonb_typeof(total_usage -> 'cached_input_tokens') = 'number' \
                    THEN (total_usage ->> 'cached_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_read, \
                CASE WHEN jsonb_typeof(total_usage -> 'cache_write_input_tokens') = 'number' \
                    THEN (total_usage ->> 'cache_write_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_write, \
                CASE WHEN jsonb_typeof(total_usage -> 'output_tokens') = 'number' \
                    THEN (total_usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output, \
                CASE WHEN jsonb_typeof(total_usage -> 'reasoning_output_tokens') = 'number' \
                    THEN (total_usage ->> 'reasoning_output_tokens')::NUMERIC::BIGINT ELSE 0 END AS reasoning, \
                CASE WHEN jsonb_typeof(total_usage -> 'total_tokens') = 'number' \
                    THEN (total_usage ->> 'total_tokens')::NUMERIC::BIGINT ELSE NULL END AS reported_total, \
                CASE WHEN jsonb_typeof(last_usage -> 'total_tokens') = 'number' \
                    THEN (last_usage ->> 'total_tokens')::NUMERIC::BIGINT ELSE NULL END AS last_total, \
                CASE WHEN jsonb_typeof(last_usage -> 'reasoning_output_tokens') = 'number' \
                    THEN (last_usage ->> 'reasoning_output_tokens')::NUMERIC::BIGINT ELSE 0 END AS last_reasoning, \
                CASE WHEN jsonb_typeof(context_window) = 'number' \
                    THEN (context_window #>> '{}')::NUMERIC::BIGINT ELSE NULL END AS reported_window \
            FROM raw \
         ) \
         INSERT INTO agent_session_usage ( \
            session_uuid, agent, input_tokens, cached_input_tokens, \
            cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens, \
            reasoning_output_tokens, total_tokens, context_tokens, model_context_window, \
            last_byte_offset, observed_at, last_usage_message_id, updated_at) \
         SELECT session_uuid, agent, \
            GREATEST(reported_input - cache_read - cache_write, 0), \
            GREATEST(cache_read, 0), GREATEST(cache_write, 0), 0, \
            GREATEST(output, 0), GREATEST(reasoning, 0), \
            GREATEST(COALESCE(reported_total, reported_input + output), 0), \
            CASE WHEN last_total IS NULL THEN NULL \
                 ELSE GREATEST(last_total - last_reasoning, 0) END, \
            CASE WHEN reported_window > 0 THEN reported_window ELSE NULL END, \
            byte_offset, timestamp, NULL, NOW() \
         FROM parsed",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let claude_sessions = sqlx::query(
        "WITH raw AS ( \
            SELECT e.session_uuid, e.agent, e.byte_offset, e.timestamp, \
                COALESCE(e.payload #>> '{message,id}', e.byte_offset::TEXT) AS response_key, \
                e.payload #>> '{message,id}' AS message_id, \
                COALESCE(e.payload #> '{message,usage}', e.payload -> 'usage') AS usage \
            FROM events e \
            WHERE e.agent = 'claude-code' AND e.payload ->> 'type' = 'assistant' \
         ), responses AS ( \
            SELECT DISTINCT ON (session_uuid, response_key) * \
            FROM raw WHERE jsonb_typeof(usage) = 'object' \
            ORDER BY session_uuid, response_key, byte_offset DESC \
         ), parsed AS ( \
            SELECT *, \
                CASE WHEN jsonb_typeof(usage -> 'input_tokens') = 'number' \
                    THEN (usage ->> 'input_tokens')::NUMERIC::BIGINT ELSE 0 END AS input, \
                CASE WHEN jsonb_typeof(usage -> 'cache_read_input_tokens') = 'number' \
                    THEN (usage ->> 'cache_read_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_read, \
                CASE WHEN jsonb_typeof(usage -> 'cache_creation_input_tokens') = 'number' \
                    THEN (usage ->> 'cache_creation_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_total, \
                CASE WHEN jsonb_typeof(usage #> '{cache_creation,ephemeral_5m_input_tokens}') = 'number' \
                    THEN (usage #>> '{cache_creation,ephemeral_5m_input_tokens}')::NUMERIC::BIGINT ELSE 0 END AS cache_5m, \
                CASE WHEN jsonb_typeof(usage #> '{cache_creation,ephemeral_1h_input_tokens}') = 'number' \
                    THEN (usage #>> '{cache_creation,ephemeral_1h_input_tokens}')::NUMERIC::BIGINT ELSE 0 END AS cache_1h, \
                CASE WHEN jsonb_typeof(usage -> 'output_tokens') = 'number' \
                    THEN (usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output, \
                CASE WHEN jsonb_typeof(usage -> 'model_context_window') = 'number' \
                    THEN (usage ->> 'model_context_window')::NUMERIC::BIGINT ELSE NULL END AS reported_window \
            FROM responses \
         ), normalized AS ( \
            SELECT *, GREATEST(cache_total - cache_1h, cache_5m, 0) AS cache_write \
            FROM parsed \
         ), totals AS ( \
            SELECT session_uuid, MIN(agent) AS agent, \
                SUM(GREATEST(input, 0))::BIGINT AS input, \
                SUM(GREATEST(cache_read, 0))::BIGINT AS cache_read, \
                SUM(GREATEST(cache_write, 0))::BIGINT AS cache_write, \
                SUM(GREATEST(cache_1h, 0))::BIGINT AS cache_write_1h, \
                SUM(GREATEST(output, 0))::BIGINT AS output, \
                MAX(byte_offset) AS last_byte_offset, \
                (ARRAY_AGG(timestamp ORDER BY byte_offset DESC))[1] AS observed_at, \
                (ARRAY_AGG(message_id ORDER BY byte_offset DESC))[1] AS last_message_id, \
                (ARRAY_AGG( \
                    GREATEST(input, 0) + GREATEST(cache_read, 0) \
                    + GREATEST(cache_write, 0) + GREATEST(cache_1h, 0) \
                    + GREATEST(output, 0) ORDER BY byte_offset DESC))[1]::BIGINT AS context_tokens, \
                (ARRAY_AGG(reported_window ORDER BY byte_offset DESC) \
                    FILTER (WHERE reported_window > 0))[1] AS model_context_window \
            FROM normalized GROUP BY session_uuid \
         ) \
         INSERT INTO agent_session_usage ( \
            session_uuid, agent, input_tokens, cached_input_tokens, \
            cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens, \
            reasoning_output_tokens, total_tokens, context_tokens, model_context_window, \
            last_byte_offset, observed_at, last_usage_message_id, updated_at) \
         SELECT session_uuid, agent, input, cache_read, cache_write, cache_write_1h, \
            output, 0, input + cache_read + cache_write + cache_write_1h + output, \
            context_tokens, model_context_window, last_byte_offset, observed_at, \
            last_message_id, NOW() \
         FROM totals",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    sqlx::query(
        "WITH codex_raw AS ( \
            SELECT DISTINCT ON (e.session_uuid, (e.timestamp AT TIME ZONE 'UTC')::DATE) \
                (e.timestamp AT TIME ZONE 'UTC')::DATE AS day, \
                e.session_uuid, e.agent, \
                e.payload #> '{payload,info,total_token_usage}' AS usage \
            FROM events e \
            WHERE e.agent = 'codex' \
              AND e.payload #>> '{payload,type}' = 'token_count' \
              AND jsonb_typeof(e.payload #> '{payload,info,total_token_usage}') = 'object' \
            ORDER BY e.session_uuid, (e.timestamp AT TIME ZONE 'UTC')::DATE, e.byte_offset DESC \
         ), codex AS ( \
            SELECT day, session_uuid, agent, \
                CASE WHEN jsonb_typeof(usage -> 'input_tokens') = 'number' \
                    THEN (usage ->> 'input_tokens')::NUMERIC::BIGINT ELSE 0 END AS reported_input, \
                CASE WHEN jsonb_typeof(usage -> 'cached_input_tokens') = 'number' \
                    THEN (usage ->> 'cached_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_read, \
                CASE WHEN jsonb_typeof(usage -> 'cache_write_input_tokens') = 'number' \
                    THEN (usage ->> 'cache_write_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_write, \
                CASE WHEN jsonb_typeof(usage -> 'output_tokens') = 'number' \
                    THEN (usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output, \
                CASE WHEN jsonb_typeof(usage -> 'reasoning_output_tokens') = 'number' \
                    THEN (usage ->> 'reasoning_output_tokens')::NUMERIC::BIGINT ELSE 0 END AS reasoning, \
                CASE WHEN jsonb_typeof(usage -> 'total_tokens') = 'number' \
                    THEN (usage ->> 'total_tokens')::NUMERIC::BIGINT ELSE NULL END AS reported_total \
            FROM codex_raw \
         ), claude_raw AS ( \
            SELECT e.session_uuid, e.agent, e.byte_offset, \
                (e.timestamp AT TIME ZONE 'UTC')::DATE AS day, \
                COALESCE(e.payload #>> '{message,id}', e.byte_offset::TEXT) AS response_key, \
                COALESCE(e.payload #> '{message,usage}', e.payload -> 'usage') AS usage \
            FROM events e \
            WHERE e.agent = 'claude-code' AND e.payload ->> 'type' = 'assistant' \
         ), claude_responses AS ( \
            SELECT DISTINCT ON (session_uuid, response_key) * \
            FROM claude_raw WHERE jsonb_typeof(usage) = 'object' \
            ORDER BY session_uuid, response_key, byte_offset DESC \
         ), claude_parsed AS ( \
            SELECT *, \
                CASE WHEN jsonb_typeof(usage -> 'input_tokens') = 'number' \
                    THEN (usage ->> 'input_tokens')::NUMERIC::BIGINT ELSE 0 END AS input, \
                CASE WHEN jsonb_typeof(usage -> 'cache_read_input_tokens') = 'number' \
                    THEN (usage ->> 'cache_read_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_read, \
                CASE WHEN jsonb_typeof(usage -> 'cache_creation_input_tokens') = 'number' \
                    THEN (usage ->> 'cache_creation_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_total, \
                CASE WHEN jsonb_typeof(usage #> '{cache_creation,ephemeral_5m_input_tokens}') = 'number' \
                    THEN (usage #>> '{cache_creation,ephemeral_5m_input_tokens}')::NUMERIC::BIGINT ELSE 0 END AS cache_5m, \
                CASE WHEN jsonb_typeof(usage #> '{cache_creation,ephemeral_1h_input_tokens}') = 'number' \
                    THEN (usage #>> '{cache_creation,ephemeral_1h_input_tokens}')::NUMERIC::BIGINT ELSE 0 END AS cache_1h, \
                CASE WHEN jsonb_typeof(usage -> 'output_tokens') = 'number' \
                    THEN (usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output \
            FROM claude_responses \
         ), claude_by_day AS ( \
            SELECT day, session_uuid, MIN(agent) AS agent, \
                SUM(GREATEST(input, 0))::BIGINT AS input, \
                SUM(GREATEST(cache_read, 0))::BIGINT AS cache_read, \
                SUM(GREATEST(cache_total - cache_1h, cache_5m, 0))::BIGINT AS cache_write, \
                SUM(GREATEST(cache_1h, 0))::BIGINT AS cache_write_1h, \
                SUM(GREATEST(output, 0))::BIGINT AS output \
            FROM claude_parsed GROUP BY day, session_uuid \
         ), claude AS ( \
            SELECT day, session_uuid, agent, \
                SUM(input) OVER w AS input, SUM(cache_read) OVER w AS cache_read, \
                SUM(cache_write) OVER w AS cache_write, \
                SUM(cache_write_1h) OVER w AS cache_write_1h, \
                SUM(output) OVER w AS output \
            FROM claude_by_day \
            WINDOW w AS (PARTITION BY session_uuid ORDER BY day ROWS UNBOUNDED PRECEDING) \
         ), snapshots AS ( \
            SELECT day, session_uuid, agent, \
                GREATEST(reported_input - cache_read - cache_write, 0)::BIGINT AS input, \
                GREATEST(cache_read, 0)::BIGINT AS cache_read, \
                GREATEST(cache_write, 0)::BIGINT AS cache_write, 0::BIGINT AS cache_write_1h, \
                GREATEST(output, 0)::BIGINT AS output, GREATEST(reasoning, 0)::BIGINT AS reasoning, \
                GREATEST(COALESCE(reported_total, reported_input + output), 0)::BIGINT AS total \
            FROM codex \
            UNION ALL \
            SELECT day, session_uuid, agent, input, cache_read, cache_write, cache_write_1h, \
                output, 0, input + cache_read + cache_write + cache_write_1h + output \
            FROM claude \
         ) \
         INSERT INTO agent_usage_daily ( \
            day, session_uuid, agent, input_tokens, cached_input_tokens, \
            cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens, \
            reasoning_output_tokens, total_tokens, updated_at) \
         SELECT day, session_uuid, agent, input, cache_read, cache_write, cache_write_1h, \
            output, reasoning, total, NOW() FROM snapshots",
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "WITH codex_raw AS ( \
            SELECT e.session_uuid, e.agent, e.byte_offset, \
                (e.timestamp AT TIME ZONE 'UTC')::DATE AS day, \
                COALESCE( \
                    (SELECT prior.payload #>> '{payload,model}' \
                       FROM events prior \
                      WHERE prior.session_uuid = e.session_uuid \
                        AND prior.byte_offset <= e.byte_offset \
                        AND prior.payload ->> 'type' = 'turn_context' \
                        AND prior.payload #>> '{payload,model}' IS NOT NULL \
                      ORDER BY prior.byte_offset DESC LIMIT 1), \
                    '(unknown model)') AS model, \
                e.payload #> '{payload,info,total_token_usage}' AS usage \
            FROM events e \
            WHERE e.agent = 'codex' \
              AND e.payload #>> '{payload,type}' = 'token_count' \
              AND jsonb_typeof(e.payload #> '{payload,info,total_token_usage}') = 'object' \
         ), codex_parsed AS ( \
            SELECT *, \
                CASE WHEN jsonb_typeof(usage -> 'input_tokens') = 'number' \
                    THEN (usage ->> 'input_tokens')::NUMERIC::BIGINT ELSE 0 END AS reported_input, \
                CASE WHEN jsonb_typeof(usage -> 'cached_input_tokens') = 'number' \
                    THEN (usage ->> 'cached_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_read, \
                CASE WHEN jsonb_typeof(usage -> 'cache_write_input_tokens') = 'number' \
                    THEN (usage ->> 'cache_write_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_write, \
                CASE WHEN jsonb_typeof(usage -> 'output_tokens') = 'number' \
                    THEN (usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output \
            FROM codex_raw \
         ), codex_normalized AS ( \
            SELECT *, GREATEST(reported_input - cache_read - cache_write, 0) AS input \
            FROM codex_parsed \
         ), codex_seq AS ( \
            SELECT *, LAG(input) OVER w AS prev_input, \
                LAG(cache_read) OVER w AS prev_cache_read, \
                LAG(cache_write) OVER w AS prev_cache_write, \
                LAG(output) OVER w AS prev_output \
            FROM codex_normalized \
            WINDOW w AS (PARTITION BY session_uuid ORDER BY byte_offset) \
         ), codex_daily AS ( \
            SELECT day, session_uuid, MIN(agent) AS agent, model, \
                SUM(GREATEST(input - COALESCE(prev_input, 0), 0))::BIGINT AS input, \
                SUM(GREATEST(cache_read - COALESCE(prev_cache_read, 0), 0))::BIGINT AS cache_read, \
                SUM(GREATEST(cache_write - COALESCE(prev_cache_write, 0), 0))::BIGINT AS cache_write, \
                0::BIGINT AS cache_write_1h, \
                SUM(GREATEST(output - COALESCE(prev_output, 0), 0))::BIGINT AS output, \
                NULL::TEXT AS last_message_id \
            FROM codex_seq GROUP BY day, session_uuid, model \
         ), claude_raw AS ( \
            SELECT e.session_uuid, e.agent, e.byte_offset, \
                (e.timestamp AT TIME ZONE 'UTC')::DATE AS day, \
                COALESCE(e.payload #>> '{message,id}', e.byte_offset::TEXT) AS response_key, \
                e.payload #>> '{message,id}' AS message_id, \
                COALESCE(e.payload #>> '{message,model}', '(unknown model)') AS model, \
                COALESCE(e.payload #> '{message,usage}', e.payload -> 'usage') AS usage \
            FROM events e \
            WHERE e.agent = 'claude-code' AND e.payload ->> 'type' = 'assistant' \
         ), claude_responses AS ( \
            SELECT DISTINCT ON (session_uuid, response_key) * \
            FROM claude_raw WHERE jsonb_typeof(usage) = 'object' \
            ORDER BY session_uuid, response_key, byte_offset DESC \
         ), claude_parsed AS ( \
            SELECT *, \
                CASE WHEN jsonb_typeof(usage -> 'input_tokens') = 'number' \
                    THEN (usage ->> 'input_tokens')::NUMERIC::BIGINT ELSE 0 END AS input, \
                CASE WHEN jsonb_typeof(usage -> 'cache_read_input_tokens') = 'number' \
                    THEN (usage ->> 'cache_read_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_read, \
                CASE WHEN jsonb_typeof(usage -> 'cache_creation_input_tokens') = 'number' \
                    THEN (usage ->> 'cache_creation_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_total, \
                CASE WHEN jsonb_typeof(usage #> '{cache_creation,ephemeral_5m_input_tokens}') = 'number' \
                    THEN (usage #>> '{cache_creation,ephemeral_5m_input_tokens}')::NUMERIC::BIGINT ELSE 0 END AS cache_5m, \
                CASE WHEN jsonb_typeof(usage #> '{cache_creation,ephemeral_1h_input_tokens}') = 'number' \
                    THEN (usage #>> '{cache_creation,ephemeral_1h_input_tokens}')::NUMERIC::BIGINT ELSE 0 END AS cache_1h, \
                CASE WHEN jsonb_typeof(usage -> 'output_tokens') = 'number' \
                    THEN (usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output \
            FROM claude_responses \
         ), claude_daily AS ( \
            SELECT day, session_uuid, MIN(agent) AS agent, model, \
                SUM(GREATEST(input, 0))::BIGINT AS input, \
                SUM(GREATEST(cache_read, 0))::BIGINT AS cache_read, \
                SUM(GREATEST(cache_total - cache_1h, cache_5m, 0))::BIGINT AS cache_write, \
                SUM(GREATEST(cache_1h, 0))::BIGINT AS cache_write_1h, \
                SUM(GREATEST(output, 0))::BIGINT AS output, \
                (ARRAY_AGG(message_id ORDER BY byte_offset DESC))[1] AS last_message_id \
            FROM claude_parsed GROUP BY day, session_uuid, model \
         ), model_days AS ( \
            SELECT * FROM codex_daily UNION ALL SELECT * FROM claude_daily \
         ) \
         INSERT INTO agent_model_usage_daily ( \
            day, session_uuid, agent, model, input_tokens, cached_input_tokens, \
            cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens, \
            last_usage_message_id, updated_at) \
         SELECT day, session_uuid, agent, model, input, cache_read, cache_write, \
            cache_write_1h, output, last_message_id, NOW() FROM model_days",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(codex_sessions.saturating_add(claude_sessions))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn extracts_codex_cumulative_spend_and_context_pressure() {
        let usage = extract_codex_usage(&json!({
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": 29_000,
                        "cached_input_tokens": 20_000,
                        "cache_write_input_tokens": 2_000,
                        "output_tokens": 3_000,
                        "reasoning_output_tokens": 1_200,
                        "total_tokens": 32_000
                    },
                    "last_token_usage": {
                        "input_tokens": 18_000,
                        "cached_input_tokens": 15_000,
                        "output_tokens": 2_000,
                        "reasoning_output_tokens": 800,
                        "total_tokens": 20_000
                    },
                    "model_context_window": 100_000
                }
            }
        }))
        .unwrap();

        assert_eq!(usage.mode, UsageMode::Cumulative);
        assert_eq!(usage.message_id, None);
        assert_eq!(usage.total_tokens, 32_000);
        assert_eq!(usage.input_tokens, 7_000);
        assert_eq!(usage.cached_input_tokens, 20_000);
        assert_eq!(usage.cache_write_input_tokens, 2_000);
        assert_eq!(usage.cache_write_1h_input_tokens, 0);
        assert_eq!(usage.context_tokens, Some(19_200));
        assert_eq!(usage.model_context_window, Some(100_000));
    }

    #[test]
    fn claude_splits_cache_reads_from_fresh_input() {
        let usage = extract_claude_usage(&json!({
            "type": "assistant",
            "message": {
                "id": "msg_abc",
                "usage": {
                    "input_tokens": 100,
                    "cache_creation_input_tokens": 2_000,
                    "cache_creation": {
                        "ephemeral_5m_input_tokens": 500,
                        "ephemeral_1h_input_tokens": 1_500
                    },
                    "cache_read_input_tokens": 7_000,
                    "output_tokens": 900
                }
            }
        }))
        .unwrap();

        assert_eq!(usage.mode, UsageMode::Delta);
        assert_eq!(usage.message_id.as_deref(), Some("msg_abc"));
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.cached_input_tokens, 7_000);
        assert_eq!(usage.cache_write_input_tokens, 500);
        assert_eq!(usage.cache_write_1h_input_tokens, 1_500);
        assert_eq!(usage.output_tokens, 900);
        assert_eq!(usage.total_tokens, 10_000);
        assert_eq!(usage.context_tokens, Some(10_000));
    }

    #[test]
    fn duplicate_content_block_lines_share_one_message_id() {
        let line = json!({
            "type": "assistant",
            "message": {
                "id": "msg_dup",
                "usage": {
                    "input_tokens": 10,
                    "cache_read_input_tokens": 90,
                    "output_tokens": 5
                }
            }
        });
        let first = extract_claude_usage(&line).unwrap();
        let second = extract_claude_usage(&line).unwrap();
        // Identical extraction — the SQL layer skips the second add because
        // the message id matches the stored last_usage_message_id.
        assert_eq!(first, second);
        assert_eq!(first.message_id.as_deref(), Some("msg_dup"));
    }
}
