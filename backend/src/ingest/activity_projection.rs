use serde_json::Value;
use uuid::Uuid;

use super::ingester::TranscriptSource;
use crate::activity::ActivityState;
use crate::db::Pool;

pub(super) async fn project_from_event_best_effort(
    pool: &Pool,
    session_uuid: Uuid,
    source: TranscriptSource,
    value: &Value,
    byte_offset: i64,
) {
    if let Err(err) = project_from_event(pool, session_uuid, source, value).await {
        tracing::warn!(
            %err,
            session = %session_uuid,
            agent = source.agent_id(),
            byte_offset,
            "agent activity projection failed",
        );
    }
}

async fn project_from_event(
    pool: &Pool,
    session_uuid: Uuid,
    source: TranscriptSource,
    value: &Value,
) -> anyhow::Result<()> {
    let transition = match source {
        TranscriptSource::ClaudeCode
            if value.get("type").and_then(Value::as_str) == Some("user") =>
        {
            Some((ActivityState::Working, first_message_text(value), "derived"))
        }
        TranscriptSource::Codex => codex_transition(value),
        _ => None,
    };
    let Some((state, summary, confidence)) = transition else {
        return Ok(());
    };
    crate::activity::set_for_current_agent_session(
        pool,
        session_uuid,
        state,
        summary.as_deref(),
        None,
        "ingester",
        confidence,
    )
    .await?;
    Ok(())
}

fn codex_transition(value: &Value) -> Option<(ActivityState, Option<String>, &'static str)> {
    let outer = super::canonical::codex_record_kind(value).unwrap_or("");
    let subtype = value
        .get("payload")
        .and_then(|payload| payload.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    match (outer, subtype) {
        ("event_msg", "task_started" | "turn_started") => {
            Some((ActivityState::Working, None, "explicit"))
        }
        ("event_msg", "task_complete" | "turn_complete") => {
            Some((ActivityState::AwaitingPrompt, None, "explicit"))
        }
        _ => None,
    }
}

fn first_message_text(value: &Value) -> Option<String> {
    let content = value.get("message")?.get("content")?;
    match content {
        Value::String(text) => non_empty_prefix(text),
        Value::Array(items) => items.iter().find_map(|item| {
            if item.get("type").and_then(Value::as_str) != Some("text") {
                return None;
            }
            item.get("text")
                .and_then(Value::as_str)
                .and_then(non_empty_prefix)
        }),
        _ => None,
    }
}

fn non_empty_prefix(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.chars().take(240).collect())
    }
}
