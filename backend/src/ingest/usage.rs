use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::ingester::TranscriptSource;

mod rebuild;
mod records;
pub(super) use rebuild::rebuild_usage_projection;

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
    last_byte_offset: i64,
    codex_response_records: bool,
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
                cache_write_1h_input_tokens, output_tokens, last_usage_message_id, \
                last_byte_offset, codex_response_records \
           FROM agent_session_usage WHERE session_uuid = $1 FOR UPDATE",
    )
    .bind(session_uuid)
    .fetch_optional(&mut **tx)
    .await?;
    if previous
        .as_ref()
        .is_some_and(|row| byte_offset <= row.last_byte_offset)
    {
        return Ok(());
    }
    let response_record = source == TranscriptSource::Codex && usage.mode == UsageMode::Delta;
    if source == TranscriptSource::Codex
        && !response_record
        && previous
            .as_ref()
            .is_some_and(|row| row.codex_response_records)
    {
        return records::update_context(tx, session_uuid, byte_offset, observed_at, &usage).await;
    }
    if response_record
        && !records::claim_response(tx, session_uuid, usage.message_id.as_deref().unwrap()).await?
    {
        return Ok(());
    }
    let daily_delta = usage.daily_delta(previous.as_ref());
    let recorded_model: Option<String> =
        if source == TranscriptSource::Codex && usage.model.is_none() {
            sqlx::query_scalar(
                "SELECT payload #>> '{payload,model}' FROM events \
             WHERE session_uuid = $1 AND byte_offset <= $2 AND kind = 'turn_context' \
               AND payload #>> '{payload,model}' IS NOT NULL \
             ORDER BY byte_offset DESC LIMIT 1",
            )
            .bind(session_uuid)
            .bind(byte_offset)
            .fetch_optional(&mut **tx)
            .await?
        } else {
            None
        };
    let model = usage
        .model
        .as_deref()
        .or(recorded_model.as_deref())
        .unwrap_or("(unknown model)");
    store_usage(
        tx,
        session_uuid,
        source,
        byte_offset,
        observed_at,
        &usage,
        response_record,
    )
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

async fn store_usage(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    source: TranscriptSource,
    byte_offset: i64,
    observed_at: DateTime<Utc>,
    usage: &UsageUpdate,
    response_record: bool,
) -> Result<(), sqlx::Error> {
    let is_delta = usage.mode == UsageMode::Delta;
    sqlx::query(
        "INSERT INTO agent_session_usage \
            (session_uuid, agent, input_tokens, cached_input_tokens, \
             cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens, \
             reasoning_output_tokens, total_tokens, context_tokens, model_context_window, \
             last_byte_offset, observed_at, last_usage_message_id, codex_response_records, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $15, $16, NOW()) \
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
            codex_response_records = agent_session_usage.codex_response_records OR $16, \
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
    .bind(response_record)
    .execute(&mut **tx)
    .await?;
    Ok(())
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
                input_tokens: self.input_tokens.max(0),
                cached_input_tokens: self.cached_input_tokens.max(0),
                cache_write_input_tokens: self.cache_write_input_tokens.max(0),
                cache_write_1h_input_tokens: self.cache_write_1h_input_tokens.max(0),
                output_tokens: self.output_tokens.max(0),
            };
        }
        // A cumulative total can legitimately shrink below the stored
        // baseline: codex compaction restarts session totals, and rows
        // written by an earlier accounting scheme can exceed the current
        // one. A negative delta violates the daily table's checks and
        // would wedge ingestion of the whole event, so floor at zero and
        // let the stored baseline reset to the new totals.
        UsageComponents {
            input_tokens: self
                .input_tokens
                .saturating_sub(previous.map_or(0, |row| row.input_tokens))
                .max(0),
            cached_input_tokens: self
                .cached_input_tokens
                .saturating_sub(previous.map_or(0, |row| row.cached_input_tokens))
                .max(0),
            cache_write_input_tokens: self
                .cache_write_input_tokens
                .saturating_sub(previous.map_or(0, |row| row.cache_write_input_tokens))
                .max(0),
            cache_write_1h_input_tokens: self
                .cache_write_1h_input_tokens
                .saturating_sub(previous.map_or(0, |row| row.cache_write_1h_input_tokens))
                .max(0),
            output_tokens: self
                .output_tokens
                .saturating_sub(previous.map_or(0, |row| row.output_tokens))
                .max(0),
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
    if super::canonical::codex_record_kind(value) == Some("token_usage_record") {
        return records::extract_response(payload);
    }
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
        // Some codex versions report input_tokens inclusive of the cache
        // categories, others not; never let the subtraction go negative.
        input_tokens: reported_input_tokens
            .saturating_sub(cached_input_tokens)
            .saturating_sub(cache_write_input_tokens)
            .max(0),
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

    /// A shrunken cumulative total (codex compaction, or baselines
    /// written under an earlier accounting scheme) must clamp the daily
    /// delta to zero — a negative component violates the daily table's
    /// checks and would wedge ingestion of the whole event.
    #[test]
    fn shrunken_cumulative_totals_clamp_daily_deltas_to_zero() {
        let update = UsageUpdate {
            mode: UsageMode::Cumulative,
            message_id: None,
            model: None,
            input_tokens: 1_000,
            cached_input_tokens: 500,
            cache_write_input_tokens: 100,
            cache_write_1h_input_tokens: 0,
            output_tokens: 200,
            reasoning_output_tokens: 0,
            total_tokens: 1_800,
            context_tokens: None,
            model_context_window: None,
        };
        let previous = StoredUsage {
            input_tokens: 50_000,
            cached_input_tokens: 40_000,
            cache_write_input_tokens: 5_000,
            cache_write_1h_input_tokens: 0,
            output_tokens: 9_000,
            last_usage_message_id: None,
            last_byte_offset: 0,
            codex_response_records: false,
        };
        let delta = update.daily_delta(Some(&previous));
        assert_eq!(delta.input_tokens, 0);
        assert_eq!(delta.cached_input_tokens, 0);
        assert_eq!(delta.cache_write_input_tokens, 0);
        assert_eq!(delta.cache_write_1h_input_tokens, 0);
        assert_eq!(delta.output_tokens, 0);
    }

    /// Codex builds that report input_tokens exclusive of cache reads
    /// would otherwise extract a negative fresh-input figure.
    #[test]
    fn codex_input_smaller_than_cache_extracts_zero_fresh_input() {
        let usage = extract_codex_usage(&json!({
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 20_000,
                        "cache_write_input_tokens": 2_000,
                        "output_tokens": 3_000,
                        "total_tokens": 26_000
                    }
                }
            }
        }))
        .unwrap();
        assert_eq!(usage.input_tokens, 0);
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
