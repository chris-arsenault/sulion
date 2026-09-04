use serde_json::Value;

use super::{content_kind_of, Block, CanonicalEvent, ContentKind, EventParser, Speaker};

pub struct CodexParser;

impl EventParser for CodexParser {
    fn agent_id(&self) -> &'static str {
        "codex"
    }

    fn parse(&self, value: &Value) -> CanonicalEvent {
        let kind = codex_record_kind(value).unwrap_or("");
        match kind {
            "response_item" => parse_codex_response_item(value),
            "event_msg" => parse_codex_event_msg(value),
            "session_meta" | "turn_context" | "compacted" | "token_usage_record" => {
                CanonicalEvent {
                    agent: self.agent_id(),
                    speaker: Speaker::System,
                    content_kind: ContentKind::None,
                    event_uuid: None,
                    parent_event_uuid: None,
                    related_tool_use_id: None,
                    is_sidechain: false,
                    is_meta: true,
                    subtype: Some(kind.to_string()),
                    blocks: Vec::new(),
                }
            }
            _ => CanonicalEvent {
                agent: self.agent_id(),
                speaker: Speaker::Other,
                content_kind: ContentKind::None,
                event_uuid: None,
                parent_event_uuid: None,
                related_tool_use_id: None,
                is_sidechain: false,
                is_meta: true,
                subtype: Some(kind.to_string()),
                blocks: vec![Block::unknown(0, value.clone())],
            },
        }
    }
}

pub fn codex_record_kind(value: &Value) -> Option<&str> {
    value
        .get("kind")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("type").and_then(|v| v.as_str()))
}

fn parse_codex_response_item(value: &Value) -> CanonicalEvent {
    let payload = value.get("payload").unwrap_or(&Value::Null);
    let subtype = payload
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("response_item");
    match subtype {
        "message" => {
            let role = payload
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("other");
            let speaker = match role {
                "user" => Speaker::User,
                "assistant" => Speaker::Assistant,
                "developer" | "system" => Speaker::System,
                _ => Speaker::Other,
            };
            let is_meta = matches!(role, "developer" | "system");
            let blocks = match payload.get("content") {
                Some(Value::Array(items)) => parse_codex_message_items(items),
                Some(Value::String(s)) => vec![Block::text(0, s.clone())],
                _ => Vec::new(),
            };
            CanonicalEvent {
                agent: "codex",
                speaker,
                content_kind: content_kind_of(&blocks),
                event_uuid: None,
                parent_event_uuid: None,
                related_tool_use_id: None,
                is_sidechain: false,
                is_meta,
                subtype: Some(subtype.to_string()),
                blocks,
            }
        }
        "reasoning" => {
            let text = codex_reasoning_text(payload).unwrap_or_default();
            let blocks = vec![Block::thinking(0, text)];
            CanonicalEvent {
                agent: "codex",
                speaker: Speaker::Assistant,
                content_kind: content_kind_of(&blocks),
                event_uuid: None,
                parent_event_uuid: None,
                related_tool_use_id: None,
                is_sidechain: false,
                is_meta: false,
                subtype: Some(subtype.to_string()),
                blocks,
            }
        }
        "function_call" | "custom_tool_call" => {
            let call_id = payload
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = payload
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(subtype)
                .to_string();
            let input = if subtype == "function_call" {
                parse_json_string(payload.get("arguments"))
            } else {
                payload.get("input").cloned().unwrap_or(Value::Null)
            };
            let blocks = vec![Block::tool_use(0, call_id, name, input)];
            CanonicalEvent {
                agent: "codex",
                speaker: Speaker::Assistant,
                content_kind: content_kind_of(&blocks),
                event_uuid: None,
                parent_event_uuid: None,
                related_tool_use_id: None,
                is_sidechain: false,
                is_meta: false,
                subtype: Some(subtype.to_string()),
                blocks,
            }
        }
        "function_call_output" | "custom_tool_call_output" => {
            let call_id = payload
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let text = payload.get("output").and_then(value_to_text);
            let is_error = payload
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let blocks = vec![Block::tool_result(0, call_id.clone(), text, is_error, None)];
            CanonicalEvent {
                agent: "codex",
                speaker: Speaker::System,
                content_kind: content_kind_of(&blocks),
                event_uuid: None,
                parent_event_uuid: None,
                related_tool_use_id: Some(call_id),
                is_sidechain: false,
                is_meta: false,
                subtype: Some(subtype.to_string()),
                blocks,
            }
        }
        _ => CanonicalEvent {
            agent: "codex",
            speaker: Speaker::System,
            content_kind: ContentKind::None,
            event_uuid: None,
            parent_event_uuid: None,
            related_tool_use_id: None,
            is_sidechain: false,
            is_meta: true,
            subtype: Some(subtype.to_string()),
            blocks: Vec::new(),
        },
    }
}

fn parse_codex_event_msg(value: &Value) -> CanonicalEvent {
    let payload = value.get("payload").unwrap_or(&Value::Null);
    let subtype = payload
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("event_msg");
    CanonicalEvent {
        agent: "codex",
        speaker: Speaker::System,
        content_kind: ContentKind::None,
        event_uuid: None,
        parent_event_uuid: None,
        related_tool_use_id: payload
            .get("call_id")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        is_sidechain: false,
        is_meta: true,
        subtype: Some(subtype.to_string()),
        blocks: Vec::new(),
    }
}

fn parse_codex_message_items(items: &[Value]) -> Vec<Block> {
    let mut blocks = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let ord = i as i32;
        let ty = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ty {
            "input_text" | "output_text" => {
                let text = item
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                blocks.push(Block::text(ord, text));
            }
            _ => blocks.push(Block::unknown(ord, item.clone())),
        }
    }
    blocks
}

fn codex_reasoning_text(payload: &Value) -> Option<String> {
    let summary = payload.get("summary")?.as_array()?;
    let mut parts = Vec::new();
    for item in summary {
        if let Some(s) = item.as_str() {
            if !s.trim().is_empty() {
                parts.push(s.to_string());
            }
            continue;
        }
        if let Some(s) = item.get("text").and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                parts.push(s.to_string());
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn parse_json_string(value: Option<&Value>) -> Value {
    let Some(Value::String(raw)) = value else {
        return value.cloned().unwrap_or(Value::Null);
    };
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.clone()))
}

fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let mut out = Vec::new();
            for part in parts {
                if let Some(text) = value_to_text(part) {
                    if !text.trim().is_empty() {
                        out.push(text);
                    }
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out.join("\n"))
            }
        }
        Value::Object(obj) => obj
            .get("text")
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
            .or_else(|| Some(Value::Object(obj.clone()).to_string())),
        other => Some(other.to_string()),
    }
}
