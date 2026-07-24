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
             last_byte_offset, observed_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW()) \
         ON CONFLICT (session_uuid) DO UPDATE SET \
            agent = EXCLUDED.agent, \
            input_tokens = CASE WHEN $12 \
                THEN agent_session_usage.input_tokens + EXCLUDED.input_tokens \
                ELSE EXCLUDED.input_tokens END, \
            cached_input_tokens = CASE WHEN $12 \
                THEN agent_session_usage.cached_input_tokens + EXCLUDED.cached_input_tokens \
                ELSE EXCLUDED.cached_input_tokens END, \
            output_tokens = CASE WHEN $12 \
                THEN agent_session_usage.output_tokens + EXCLUDED.output_tokens \
                ELSE EXCLUDED.output_tokens END, \
            reasoning_output_tokens = CASE WHEN $12 \
                THEN agent_session_usage.reasoning_output_tokens + EXCLUDED.reasoning_output_tokens \
                ELSE EXCLUDED.reasoning_output_tokens END, \
            total_tokens = CASE WHEN $12 \
                THEN agent_session_usage.total_tokens + EXCLUDED.total_tokens \
                ELSE EXCLUDED.total_tokens END, \
            context_tokens = COALESCE(EXCLUDED.context_tokens, agent_session_usage.context_tokens), \
            model_context_window = COALESCE( \
                EXCLUDED.model_context_window, agent_session_usage.model_context_window \
            ), \
            last_byte_offset = EXCLUDED.last_byte_offset, \
            observed_at = EXCLUDED.observed_at, \
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
        input_tokens: token_at(total, "input_tokens"),
        cached_input_tokens: token_at(total, "cached_input_tokens"),
        output_tokens: token_at(total, "output_tokens"),
        reasoning_output_tokens,
        total_tokens,
        context_tokens: last.map(|_| last_total.saturating_sub(last_reasoning)),
        model_context_window: positive_i64_at(info, &["model_context_window"]),
    })
}

fn extract_claude_usage(value: &Value) -> Option<UsageUpdate> {
    if string_at(value, &["type"]) != Some("assistant") {
        return None;
    }
    let usage = value
        .get("message")
        .and_then(|message| message.get("usage"))
        .or_else(|| value.get("usage"))?;
    if !usage.is_object() {
        return None;
    }
    let input_tokens = token_at(usage, "input_tokens");
    let output_tokens = token_at(usage, "output_tokens");
    let cached_input_tokens = token_at(usage, "cache_creation_input_tokens")
        .saturating_add(token_at(usage, "cache_read_input_tokens"));
    let total_tokens = input_tokens
        .saturating_add(cached_input_tokens)
        .saturating_add(output_tokens);
    Some(UsageUpdate {
        mode: UsageMode::Delta,
        input_tokens,
        cached_input_tokens,
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
        assert_eq!(usage.total_tokens, 32_000);
        assert_eq!(usage.cached_input_tokens, 20_000);
        assert_eq!(usage.context_tokens, Some(19_200));
        assert_eq!(usage.model_context_window, Some(100_000));
    }

    #[test]
    fn extracts_claude_cache_aware_delta_and_context() {
        let usage = extract_claude_usage(&json!({
            "type": "assistant",
            "message": {
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
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.cached_input_tokens, 9_000);
        assert_eq!(usage.output_tokens, 900);
        assert_eq!(usage.total_tokens, 10_000);
        assert_eq!(usage.context_tokens, Some(10_000));
    }
}
