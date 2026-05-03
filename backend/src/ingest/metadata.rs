use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::db::Pool;

#[derive(Default)]
struct MetadataPatch {
    model: Option<String>,
    model_provider: Option<String>,
    reasoning_effort: Option<String>,
    cli_version: Option<String>,
    cwd: Option<String>,
    model_context_window: Option<i64>,
    raw: Map<String, Value>,
}

impl MetadataPatch {
    fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.model_provider.is_none()
            && self.reasoning_effort.is_none()
            && self.cli_version.is_none()
            && self.cwd.is_none()
            && self.model_context_window.is_none()
            && self.raw.is_empty()
    }
}

pub async fn upsert_from_event(
    pool: &Pool,
    session_uuid: Uuid,
    agent: &str,
    value: &Value,
) -> anyhow::Result<()> {
    let patch = extract_metadata(agent, value);
    if patch.is_empty() {
        return Ok(());
    }

    sqlx::query(
        "INSERT INTO agent_session_metadata \
            (session_uuid, agent, model, model_provider, reasoning_effort, cli_version, cwd, \
             model_context_window, raw_metadata_json, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW()) \
         ON CONFLICT (session_uuid) DO UPDATE SET \
            agent = EXCLUDED.agent, \
            model = COALESCE(EXCLUDED.model, agent_session_metadata.model), \
            model_provider = COALESCE(EXCLUDED.model_provider, agent_session_metadata.model_provider), \
            reasoning_effort = COALESCE(EXCLUDED.reasoning_effort, agent_session_metadata.reasoning_effort), \
            cli_version = COALESCE(EXCLUDED.cli_version, agent_session_metadata.cli_version), \
            cwd = COALESCE(EXCLUDED.cwd, agent_session_metadata.cwd), \
            model_context_window = COALESCE(EXCLUDED.model_context_window, agent_session_metadata.model_context_window), \
            raw_metadata_json = agent_session_metadata.raw_metadata_json || EXCLUDED.raw_metadata_json, \
            updated_at = NOW()",
    )
    .bind(session_uuid)
    .bind(agent)
    .bind(patch.model)
    .bind(patch.model_provider)
    .bind(patch.reasoning_effort)
    .bind(patch.cli_version)
    .bind(patch.cwd)
    .bind(patch.model_context_window)
    .bind(Value::Object(patch.raw))
    .execute(pool)
    .await?;

    Ok(())
}

fn extract_metadata(agent: &str, value: &Value) -> MetadataPatch {
    match agent {
        "codex" => extract_codex_metadata(value),
        _ => extract_claude_metadata(value),
    }
}

fn extract_codex_metadata(value: &Value) -> MetadataPatch {
    let mut patch = MetadataPatch::default();
    let kind = super::canonical::codex_record_kind(value).unwrap_or("");
    let payload = value.get("payload").unwrap_or(&Value::Null);

    match kind {
        "session_meta" => {
            patch.model_provider = string_at(payload, &["model_provider"]);
            patch.cli_version = string_at(payload, &["cli_version"]);
            patch.cwd = string_at(payload, &["cwd"]);
            patch
                .raw
                .insert("session_meta".to_string(), compact_object(payload));
        }
        "turn_context" => {
            patch.model = string_at(payload, &["model"]);
            patch.reasoning_effort = string_at(payload, &["reasoning_effort"])
                .or_else(|| string_at(payload, &["effort"]));
            patch.cwd = string_at(payload, &["cwd"]);
            patch
                .raw
                .insert("turn_context".to_string(), compact_object(payload));
        }
        "event_msg" if string_at(payload, &["type"]).as_deref() == Some("task_started") => {
            patch.model_context_window = i64_at(payload, &["model_context_window"]);
            patch.raw.insert(
                "task_started".to_string(),
                json!({
                    "model_context_window": patch.model_context_window,
                    "collaboration_mode_kind": string_at(payload, &["collaboration_mode_kind"]),
                }),
            );
        }
        "event_msg"
            if string_at(payload, &["type"]).as_deref() == Some("collab_agent_spawn_end") =>
        {
            patch.model = string_at(payload, &["model"]);
            patch.reasoning_effort = string_at(payload, &["reasoning_effort"])
                .or_else(|| string_at(payload, &["effort"]));
            patch.raw.insert(
                "collab_agent_spawn_end".to_string(),
                json!({
                    "model": patch.model.as_deref(),
                    "reasoning_effort": patch.reasoning_effort.as_deref(),
                }),
            );
        }
        _ => {}
    }

    patch
}

fn extract_claude_metadata(value: &Value) -> MetadataPatch {
    let mut patch = MetadataPatch {
        model: string_at(value, &["message", "model"]).or_else(|| string_at(value, &["model"])),
        cli_version: string_at(value, &["version"]).or_else(|| string_at(value, &["cli_version"])),
        cwd: string_at(value, &["cwd"]),
        ..MetadataPatch::default()
    };
    if patch.model.is_some() || patch.cli_version.is_some() || patch.cwd.is_some() {
        patch.raw.insert(
            "claude".to_string(),
            json!({
                "type": string_at(value, &["type"]),
                "model": patch.model.as_deref(),
                "version": patch.cli_version.as_deref(),
                "cwd": patch.cwd.as_deref(),
            }),
        );
    }
    patch
}

fn compact_object(value: &Value) -> Value {
    if !value.is_object() {
        return value.clone();
    }
    json!({
        "id": string_at(value, &["id"]),
        "cwd": string_at(value, &["cwd"]),
        "model": string_at(value, &["model"]),
        "model_provider": string_at(value, &["model_provider"]),
        "cli_version": string_at(value, &["cli_version"]),
        "effort": string_at(value, &["effort"]),
        "reasoning_effort": string_at(value, &["reasoning_effort"]),
    })
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().map(ToString::to_string)
}

fn i64_at(value: &Value, path: &[&str]) -> Option<i64> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_i64()
        .or_else(|| current.as_u64().and_then(|value| i64::try_from(value).ok()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn extracts_codex_turn_metadata() {
        let patch = extract_codex_metadata(&json!({
            "type": "turn_context",
            "payload": {
                "cwd": "/repo",
                "model": "gpt-5.4",
                "effort": "medium"
            }
        }));
        assert_eq!(patch.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(patch.reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(patch.cwd.as_deref(), Some("/repo"));
    }

    #[test]
    fn extracts_claude_message_model() {
        let patch = extract_claude_metadata(&json!({
            "type": "assistant",
            "message": { "model": "claude-sonnet-4" }
        }));
        assert_eq!(patch.model.as_deref(), Some("claude-sonnet-4"));
    }
}
