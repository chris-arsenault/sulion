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
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
    context_tokens: Option<i64>,
    model_context_window: Option<i64>,
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
    let is_delta = usage.mode == UsageMode::Delta;
    sqlx::query(
        "INSERT INTO agent_session_usage \
            (session_uuid, agent, input_tokens, cached_input_tokens, output_tokens, \
             reasoning_output_tokens, total_tokens, context_tokens, model_context_window, \
             last_byte_offset, observed_at, last_usage_message_id, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $13, NOW()) \
         ON CONFLICT (session_uuid) DO UPDATE SET \
            agent = EXCLUDED.agent, \
            input_tokens = CASE \
                WHEN NOT $12 THEN EXCLUDED.input_tokens \
                WHEN $13::TEXT IS NOT NULL \
                    AND $13 = agent_session_usage.last_usage_message_id \
                    THEN agent_session_usage.input_tokens \
                ELSE agent_session_usage.input_tokens + EXCLUDED.input_tokens END, \
            cached_input_tokens = CASE \
                WHEN NOT $12 THEN EXCLUDED.cached_input_tokens \
                WHEN $13::TEXT IS NOT NULL \
                    AND $13 = agent_session_usage.last_usage_message_id \
                    THEN agent_session_usage.cached_input_tokens \
                ELSE agent_session_usage.cached_input_tokens + EXCLUDED.cached_input_tokens END, \
            output_tokens = CASE \
                WHEN NOT $12 THEN EXCLUDED.output_tokens \
                WHEN $13::TEXT IS NOT NULL \
                    AND $13 = agent_session_usage.last_usage_message_id \
                    THEN agent_session_usage.output_tokens \
                ELSE agent_session_usage.output_tokens + EXCLUDED.output_tokens END, \
            reasoning_output_tokens = CASE \
                WHEN NOT $12 THEN EXCLUDED.reasoning_output_tokens \
                WHEN $13::TEXT IS NOT NULL \
                    AND $13 = agent_session_usage.last_usage_message_id \
                    THEN agent_session_usage.reasoning_output_tokens \
                ELSE agent_session_usage.reasoning_output_tokens \
                    + EXCLUDED.reasoning_output_tokens END, \
            total_tokens = CASE \
                WHEN NOT $12 THEN EXCLUDED.total_tokens \
                WHEN $13::TEXT IS NOT NULL \
                    AND $13 = agent_session_usage.last_usage_message_id \
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
    snapshot_daily(tx, session_uuid, observed_at).await
}

/// End-of-day cumulative snapshot: copy the session's current totals into
/// the (day, session) row. Daily deltas are derived at read time from
/// consecutive snapshots, keeping the hot path free of delta bookkeeping.
async fn snapshot_daily(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    observed_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO agent_usage_daily \
            (day, session_uuid, agent, input_tokens, cached_input_tokens, output_tokens, \
             reasoning_output_tokens, total_tokens, updated_at) \
         SELECT $2::DATE, session_uuid, agent, input_tokens, cached_input_tokens, \
                output_tokens, reasoning_output_tokens, total_tokens, NOW() \
         FROM agent_session_usage WHERE session_uuid = $1 \
         ON CONFLICT (day, session_uuid) DO UPDATE SET \
            agent = EXCLUDED.agent, \
            input_tokens = EXCLUDED.input_tokens, \
            cached_input_tokens = EXCLUDED.cached_input_tokens, \
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
    let total_tokens = total_token_count(total);
    let last_total = last.map_or(0, total_token_count);
    let last_reasoning = last.map_or(0, |usage| token_at(usage, "reasoning_output_tokens"));
    Some(UsageUpdate {
        mode: UsageMode::Cumulative,
        message_id: None,
        input_tokens: token_at(total, "input_tokens"),
        cached_input_tokens: token_at(total, "cached_input_tokens"),
        output_tokens: token_at(total, "output_tokens"),
        reasoning_output_tokens,
        total_tokens,
        context_tokens: last.map(|_| last_total.saturating_sub(last_reasoning)),
        model_context_window: positive_i64_at(info, &["model_context_window"]),
    })
}

/// Claude usage normalization. `cached_input_tokens` means "served from
/// cache" (reads only) so `total - cached` is comparable fresh work across
/// agents; cache *creation* is genuinely new prompt work and folds into
/// `input_tokens`.
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
    let cache_creation = token_at(usage, "cache_creation_input_tokens");
    let cache_read = token_at(usage, "cache_read_input_tokens");
    let input_tokens = token_at(usage, "input_tokens").saturating_add(cache_creation);
    let output_tokens = token_at(usage, "output_tokens");
    let total_tokens = input_tokens
        .saturating_add(cache_read)
        .saturating_add(output_tokens);
    Some(UsageUpdate {
        mode: UsageMode::Delta,
        message_id,
        input_tokens,
        cached_input_tokens: cache_read,
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
        assert_eq!(usage.cached_input_tokens, 20_000);
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
                    "cache_read_input_tokens": 7_000,
                    "output_tokens": 900
                }
            }
        }))
        .unwrap();

        assert_eq!(usage.mode, UsageMode::Delta);
        assert_eq!(usage.message_id.as_deref(), Some("msg_abc"));
        // cache creation folds into fresh input; cached means reads only.
        assert_eq!(usage.input_tokens, 2_100);
        assert_eq!(usage.cached_input_tokens, 7_000);
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
