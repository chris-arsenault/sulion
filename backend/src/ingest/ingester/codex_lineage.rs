//! Codex lineage: the session, parent and turn a Codex record belongs to,
//! carried across lines and ticks in `CodexSessionContext`.

use serde_json::Value;
use uuid::Uuid;

use crate::db::Pool;
use crate::ingest::canonical::{codex_record_kind, CanonicalEvent, ContentKind, Speaker};

#[derive(Debug, Clone)]
pub(super) struct CodexSessionContext {
    session_id: String,
    parent_session_id: Option<String>,
    current_turn_id: Option<String>,
    history_start_ordinal: Option<u64>,
}

impl CodexSessionContext {
    pub(super) fn new(session_uuid: Uuid) -> Self {
        Self {
            session_id: session_uuid.to_string(),
            parent_session_id: None,
            current_turn_id: None,
            history_start_ordinal: None,
        }
    }

    fn is_inherited(&self, value: &Value) -> bool {
        let foreign_meta = codex_record_kind(value) == Some("session_meta")
            && value
                .pointer("/payload/id")
                .and_then(Value::as_str)
                .is_some_and(|id| id != self.session_id);
        foreign_meta
            || self
                .history_start_ordinal
                .zip(value.get("ordinal").and_then(Value::as_u64))
                .is_some_and(|(start, ordinal)| ordinal > 0 && ordinal < start)
    }
}

pub(super) async fn load_codex_context(
    pool: &Pool,
    session_uuid: Uuid,
) -> anyhow::Result<CodexSessionContext> {
    let mut ctx = CodexSessionContext::new(session_uuid);

    let meta_row: Option<(Value,)> = sqlx::query_as(
        "SELECT payload \
         FROM events \
         WHERE session_uuid = $1 AND agent = 'codex' AND kind = 'session_meta' \
           AND payload #>> '{payload,id}' = $1::TEXT \
         ORDER BY byte_offset ASC \
         LIMIT 1",
    )
    .bind(session_uuid)
    .fetch_optional(pool)
    .await?;
    if let Some((payload,)) = meta_row {
        update_codex_context(&mut ctx, &payload, session_uuid);
    }

    let turn_row: Option<(Value,)> = sqlx::query_as(
        "SELECT payload \
         FROM events \
         WHERE session_uuid = $1 AND agent = 'codex' AND kind IN ('turn_context', 'task_started') \
           AND subtype IS DISTINCT FROM 'inherited_history' \
         ORDER BY byte_offset DESC \
         LIMIT 1",
    )
    .bind(session_uuid)
    .fetch_optional(pool)
    .await?;
    if let Some((payload,)) = turn_row {
        update_codex_context(&mut ctx, &payload, session_uuid);
    }

    Ok(ctx)
}

pub(super) fn enrich_codex_lineage(
    parsed: &mut CanonicalEvent,
    value: &Value,
    session_uuid: Uuid,
    byte_offset: i64,
    codex_ctx: Option<&CodexSessionContext>,
) {
    let payload = value.get("payload").unwrap_or(&Value::Null);
    let outer = codex_record_kind(value).unwrap_or("");
    let subtype = parsed.subtype.as_deref().unwrap_or("");
    let session_id = codex_ctx
        .map(|ctx| ctx.session_id.clone())
        .unwrap_or_else(|| session_uuid.to_string());
    let session_parent = codex_ctx.and_then(|ctx| ctx.parent_session_id.clone());
    let current_turn_id = codex_ctx.and_then(|ctx| ctx.current_turn_id.clone());
    let synthetic_id = format!("codex:{session_uuid}:{byte_offset}");

    if codex_ctx.is_some_and(|ctx| ctx.is_inherited(value)) {
        parsed.event_uuid = Some(synthetic_id);
        parsed.parent_event_uuid = None;
        parsed.related_tool_use_id = None;
        parsed.is_sidechain = session_parent.is_some();
        parsed.is_meta = true;
        parsed.speaker = Speaker::System;
        parsed.content_kind = ContentKind::None;
        parsed.subtype = Some("inherited_history".into());
        parsed.blocks.clear();
        return;
    }

    match outer {
        "session_meta" => {
            parsed.event_uuid = codex_string_at_path(payload, &["id"])
                .map(ToString::to_string)
                .or(Some(session_id.clone()));
            parsed.parent_event_uuid = codex_parent_session_string(value).map(ToString::to_string);
            parsed.is_sidechain = parsed.parent_event_uuid.is_some();
        }
        "turn_context" => {
            parsed.event_uuid = codex_string_at_path(payload, &["turn_id"])
                .map(ToString::to_string)
                .or(Some(synthetic_id));
            parsed.parent_event_uuid = Some(session_id);
            parsed.is_sidechain = session_parent.is_some();
        }
        "response_item" => {
            let parent = current_turn_id.or(Some(session_id));
            parsed.parent_event_uuid = parent;
            parsed.is_sidechain = session_parent.is_some();
            parsed.event_uuid = match subtype {
                "agent_message" => payload
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or(Some(synthetic_id)),
                "function_call" | "custom_tool_call" => parsed
                    .blocks
                    .iter()
                    .find_map(|block| block.tool_id.clone())
                    .or(Some(synthetic_id)),
                "function_call_output" | "custom_tool_call_output" => parsed
                    .related_tool_use_id
                    .as_ref()
                    .map(|id| format!("{id}:output:{byte_offset}"))
                    .or(Some(synthetic_id)),
                _ => Some(synthetic_id),
            };
        }
        "event_msg" => {
            parsed.related_tool_use_id = parsed
                .related_tool_use_id
                .clone()
                .or_else(|| codex_string_at_path(payload, &["call_id"]).map(ToString::to_string));
            parsed.is_sidechain = session_parent.is_some();
            match subtype {
                "item_completed"
                    if payload.pointer("/item/type").and_then(Value::as_str)
                        == Some("SubAgentActivity")
                        && payload.pointer("/item/kind").and_then(Value::as_str)
                            == Some("started") =>
                {
                    parsed.event_uuid = payload
                        .pointer("/item/agent_thread_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or(Some(synthetic_id));
                    parsed.related_tool_use_id = payload
                        .pointer("/item/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    parsed.parent_event_uuid = current_turn_id.or(Some(session_id));
                    parsed.is_sidechain = false;
                }
                "task_started" => {
                    parsed.event_uuid = codex_string_at_path(payload, &["turn_id"])
                        .map(ToString::to_string)
                        .or(Some(synthetic_id));
                    parsed.parent_event_uuid = Some(session_id);
                }
                "collab_agent_spawn_end" => {
                    parsed.event_uuid = codex_string_at_path(payload, &["new_thread_id"])
                        .map(ToString::to_string)
                        .or(Some(synthetic_id));
                    parsed.parent_event_uuid = current_turn_id.or(Some(session_id));
                    parsed.is_sidechain = false;
                }
                _ => {
                    parsed.event_uuid = Some(synthetic_id);
                    parsed.parent_event_uuid = codex_string_at_path(payload, &["turn_id"])
                        .map(ToString::to_string)
                        .or(current_turn_id)
                        .or(Some(session_id));
                }
            }
        }
        _ => {}
    }
}

pub(super) fn update_codex_context(
    ctx: &mut CodexSessionContext,
    value: &Value,
    session_uuid: Uuid,
) {
    if ctx.is_inherited(value) {
        return;
    }
    let payload = value.get("payload").unwrap_or(&Value::Null);
    match codex_record_kind(value).unwrap_or("") {
        "session_meta" => {
            ctx.session_id = codex_string_at_path(payload, &["id"])
                .map(ToString::to_string)
                .unwrap_or_else(|| session_uuid.to_string());
            ctx.parent_session_id = codex_parent_session_string(value).map(ToString::to_string);
            ctx.history_start_ordinal = payload
                .get("subagent_history_start_ordinal")
                .and_then(Value::as_u64);
        }
        "turn_context" => {
            ctx.current_turn_id =
                codex_string_at_path(payload, &["turn_id"]).map(ToString::to_string);
        }
        "event_msg" if payload.get("type").and_then(|v| v.as_str()) == Some("task_started") => {
            ctx.current_turn_id =
                codex_string_at_path(payload, &["turn_id"]).map(ToString::to_string);
        }
        _ => {}
    }
}

fn codex_parent_session_string(value: &Value) -> Option<&str> {
    let payload = value.get("payload").unwrap_or(&Value::Null);
    codex_string_at_path(payload, &["forked_from_id"]).or_else(|| {
        codex_string_at_path(
            payload,
            &["source", "subagent", "thread_spawn", "parent_thread_id"],
        )
    })
}

pub(super) fn detect_codex_parent_session(value: &Value, current: Uuid) -> Option<Uuid> {
    if value.pointer("/payload/id").and_then(Value::as_str) != Some(current.to_string().as_str()) {
        return None;
    }
    codex_parent_session_string(value)
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .filter(|uuid| *uuid != current)
}

fn codex_string_at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str()
}
