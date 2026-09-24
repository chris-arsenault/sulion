//! The turn digest: the readable record of a turn stored in
//! `timeline_turns.markdown`, served to agents by `sulion-retrieve turn`
//! and offered as "copy turn as markdown" in the timeline.
//!
//! It holds the prompt, every assistant text block, and one header line
//! per tool call naming the tool and its one-line summary. Tool inputs,
//! diffs, and result bodies are deliberately not part of it: they already
//! live in `timeline_operations` and `event_blocks`, where the turn-detail
//! API and search read them, and a digest that embedded them ran to tens
//! of kilobytes per turn and was rewritten every time a live turn grew.

use std::collections::HashMap;

use serde_json::Value;

use super::{TimelineAssistantItem, TimelineChunk, TimelineItem, TimelineToolPair};

/// The digest of a stored turn: its prompt, then each item's assistant text
/// and tool calls, and agent messages, in order.
pub(crate) fn compose_turn_markdown(
    user_prompt_text: Option<&str>,
    items: &[TimelineItem],
    pairs: &[TimelineToolPair],
) -> String {
    let pair_by_id: HashMap<&str, &TimelineToolPair> =
        pairs.iter().map(|pair| (pair.id.as_str(), pair)).collect();
    let mut parts = Vec::new();
    if let Some(text) = user_prompt_text.filter(|text| !text.trim().is_empty()) {
        parts.push(format_prompt(text));
    }
    for item in items {
        match &item.chunk {
            TimelineChunk::Assistant { items, .. } => {
                for part in items {
                    match part {
                        TimelineAssistantItem::Text { text } => parts.push(text.trim().to_string()),
                        TimelineAssistantItem::Tool { pair_id } => {
                            if let Some(pair) = pair_by_id.get(pair_id.as_str()) {
                                parts.push(format_tool_pair_markdown(pair));
                            }
                        }
                    }
                }
            }
            TimelineChunk::System {
                subtype: Some(subtype),
                text,
                ..
            } if subtype == "agent_message" => parts.push(text.trim().to_string()),
            _ => {}
        }
    }
    parts.retain(|part| !part.is_empty());
    parts.join("\n\n")
}

fn format_prompt(text: &str) -> String {
    let quoted = text
        .split('\n')
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("**Prompt**\n\n{quoted}")
}

/// One line per tool call: the operation, its one-line summary, and its
/// pending or error state.
fn format_tool_pair_markdown(pair: &TimelineToolPair) -> String {
    format!(
        "**Tool:** `{}`{}{}",
        pair_operation_type(pair),
        tool_one_line(pair),
        tool_status(pair)
    )
}

fn tool_status(pair: &TimelineToolPair) -> String {
    if pair.is_pending {
        " _(pending)_".to_string()
    } else if pair.is_error {
        " _(error)_".to_string()
    } else {
        String::new()
    }
}

fn tool_one_line(pair: &TimelineToolPair) -> String {
    let Some(Value::Object(input)) = &pair.input else {
        return String::new();
    };
    let pick = |key: &str| input.get(key).and_then(Value::as_str);
    let summary = match pair_operation_type(pair) {
        "edit" | "write" | "multi_edit" | "read" => pick("path").unwrap_or_default(),
        "bash" | "exec" => pick("description")
            .or_else(|| pick("command"))
            .unwrap_or_default(),
        "exec_command" => pick("cmd").or_else(|| pick("command")).unwrap_or_default(),
        "grep" | "glob" => pick("pattern").unwrap_or_default(),
        "task" => pick("description")
            .or_else(|| pick("agent"))
            .unwrap_or_default(),
        "web_fetch" => pick("url").unwrap_or_default(),
        "web_search" => pick("query").unwrap_or_default(),
        _ => pick("command").or_else(|| pick("cmd")).unwrap_or_default(),
    };
    // Pairs that canonicalised into `file_edits` (and nothing above
    // matched — e.g. a code-mode exec that only applies a patch)
    // summarize by the first edited path.
    let summary = if summary.is_empty() {
        input
            .get("file_edits")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("path"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else {
        summary
    };
    if summary.is_empty() {
        String::new()
    } else {
        format!(" `{}`", truncate(summary, 160))
    }
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max {
        text.to_string()
    } else {
        let keep = max.saturating_sub(1);
        let shortened: String = text.chars().take(keep).collect();
        format!("{shortened}…")
    }
}

pub(crate) fn pair_operation_type(pair: &TimelineToolPair) -> &str {
    pair.operation_type.as_deref().unwrap_or(pair.name.as_str())
}

pub(crate) fn subagent_title(pair: &TimelineToolPair) -> String {
    let Some(Value::Object(input)) = &pair.input else {
        return "Agent log".to_string();
    };
    if let Some(description) = input.get("description").and_then(Value::as_str) {
        return format!("Agent log · {description}");
    }
    if let Some(agent) = input.get("agent").and_then(Value::as_str) {
        return format!("Agent log · {agent}");
    }
    "Agent log".to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ingest::timeline::TimelineToolResult;

    #[test]
    fn truncate_handles_multibyte_boundaries() {
        let text = "a".repeat(1498) + "—tail";
        let truncated = truncate(&text, 1500);
        assert!(truncated.ends_with('…'));
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn tool_digest_is_one_header_line_without_input_or_result() {
        let pair = TimelineToolPair {
            id: "t1".to_string(),
            name: "Edit".to_string(),
            raw_name: None,
            operation_type: Some("edit".to_string()),
            category: None,
            input: Some(json!({
                "path": "src/lib.rs",
                "file_edits": [{ "path": "src/lib.rs", "operation": "update",
                    "in_out": { "old_text": "fn old() {}", "new_text": "fn main() {}" } }]
            })),
            result: Some(TimelineToolResult {
                content: Some("edited 1 file; here is a long body".repeat(50)),
                payload: None,
                is_error: true,
            }),
            is_error: true,
            is_pending: false,
            file_touches: Vec::new(),
            subagent: None,
        };
        let digest = format_tool_pair_markdown(&pair);
        assert_eq!(digest, "**Tool:** `edit` `src/lib.rs` _(error)_");
        assert!(!digest.contains("fn main"));
        assert!(!digest.contains("long body"));
    }
}
