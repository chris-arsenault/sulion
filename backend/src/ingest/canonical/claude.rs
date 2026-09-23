use serde_json::Value;

use super::{
    canonicalize_tool_result_payload, content_kind_of, Block, CanonicalEvent, EventParser, Speaker,
};

pub struct ClaudeParser;

impl EventParser for ClaudeParser {
    fn agent_id(&self) -> &'static str {
        "claude-code"
    }

    fn parse(&self, value: &Value) -> CanonicalEvent {
        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let attachment = value.get("attachment");
        let queued_human = kind == "attachment"
            && attachment.is_some_and(|a| {
                a.get("type").and_then(Value::as_str) == Some("queued_command")
                    && a.get("commandMode").and_then(Value::as_str) == Some("prompt")
                    && (a.pointer("/origin/kind").and_then(Value::as_str) == Some("human")
                        || a.get("humanTurn").and_then(Value::as_bool) == Some(true))
            });
        let speaker = match kind {
            "attachment" if queued_human => Speaker::User,
            "attachment" => Speaker::System,
            "user" => Speaker::User,
            "assistant" => Speaker::Assistant,
            "system" => Speaker::System,
            "summary" => Speaker::Summary,
            _ => Speaker::Other,
        };

        // Most content lives under `message.content`. When `content` is
        // a string rather than an array (happens on some user events),
        // treat the whole string as one text block.
        let content = if queued_human {
            attachment.and_then(|a| a.get("prompt"))
        } else {
            value.get("message").and_then(|m| m.get("content"))
        };

        let tool_result_payload = value
            .get("toolUseResult")
            .cloned()
            .filter(|payload| !payload.is_null())
            .map(canonicalize_tool_result_payload);

        let mut blocks = match content {
            Some(Value::Array(arr)) => parse_blocks(arr, tool_result_payload.as_ref()),
            Some(Value::String(s)) => vec![Block::text(0, s.clone())],
            _ => Vec::new(),
        };
        if blocks.is_empty() && kind == "summary" {
            if let Some(summary) = value.get("summary").and_then(|v| v.as_str()) {
                blocks.push(Block::text(0, summary.to_string()));
            }
        }
        if kind == "attachment" && !queued_human {
            if let Some(rendered) = value.get("rendered").and_then(Value::as_array) {
                blocks.extend(
                    rendered
                        .iter()
                        .filter_map(|part| part.get("content").and_then(Value::as_str))
                        .enumerate()
                        .map(|(i, text)| Block::text(i as i32, text)),
                );
            }
        }
        if speaker == Speaker::User {
            for block in &mut blocks {
                if block.kind == super::BlockKind::Text {
                    if let Some(text) = &mut block.text {
                        *text = super::claude_text::normalize_user_text(text);
                    }
                }
            }
        }

        let content_kind = content_kind_of(&blocks);

        CanonicalEvent {
            agent: self.agent_id(),
            speaker,
            content_kind,
            event_uuid: queued_human
                .then(|| attachment.and_then(|a| string_field(a, &["source_uuid"])))
                .flatten()
                .or_else(|| string_field(value, &["uuid"])),
            parent_event_uuid: string_field(value, &["parentUuid", "parent_uuid"]),
            related_tool_use_id: string_field(value, &["tool_use_id"]),
            is_sidechain: bool_field(value, &["isSidechain"]).unwrap_or(false),
            is_meta: if queued_human {
                false
            } else {
                kind == "attachment" || bool_field(value, &["isMeta"]).unwrap_or(false)
            },
            subtype: if queued_human {
                Some("queued_user_prompt".into())
            } else {
                string_field(value, &["subtype"])
                    .or_else(|| attachment.and_then(|a| string_field(a, &["type"])))
            },
            blocks,
        }
    }
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_str()))
        .map(ToString::to_string)
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_bool()))
}

fn parse_blocks(arr: &[Value], tool_result_payload: Option<&Value>) -> Vec<Block> {
    let mut out = Vec::with_capacity(arr.len());
    let mut attached_tool_result_payload = false;
    for (i, raw) in arr.iter().enumerate() {
        let ord = i as i32;
        let Some(ty) = raw.get("type").and_then(|v| v.as_str()) else {
            out.push(Block::unknown(ord, raw.clone()));
            continue;
        };
        match ty {
            "text" => {
                let text = raw
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(Block::text(ord, text));
            }
            "thinking" => {
                let text = raw
                    .get("thinking")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(Block::thinking(ord, text));
            }
            "tool_use" => {
                let id = raw
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = raw
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input = raw.get("input").cloned().unwrap_or(Value::Null);
                out.push(Block::tool_use(ord, id, name, input));
            }
            "tool_result" => {
                let id = raw
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let is_error = raw
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                // `content` here can be a string, an array of {type:text, text}
                // blocks, or an object — flatten to a string.
                let text = match raw.get("content") {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Array(parts)) => {
                        let mut buf = String::new();
                        for p in parts {
                            if let Some(s) = p.get("text").and_then(|v| v.as_str()) {
                                if !buf.is_empty() {
                                    buf.push('\n');
                                }
                                buf.push_str(s);
                            }
                        }
                        if buf.is_empty() {
                            None
                        } else {
                            Some(buf)
                        }
                    }
                    Some(other) => Some(other.to_string()),
                    None => None,
                };
                let tool_output = if attached_tool_result_payload {
                    None
                } else {
                    attached_tool_result_payload = tool_result_payload.is_some();
                    tool_result_payload.cloned()
                };
                out.push(Block::tool_result(ord, id, text, is_error, tool_output));
            }
            _ => {
                out.push(Block::unknown(ord, raw.clone()));
            }
        }
    }
    out
}
