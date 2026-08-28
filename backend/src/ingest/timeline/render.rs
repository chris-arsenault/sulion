use std::collections::HashMap;

use serde_json::Value;

use crate::ingest::canonical::BlockKind;

use super::project::{is_assistant_event, is_tool_result_event, user_prompt_text};
use super::{StoredEvent, TimelineToolPair};

pub(crate) fn format_turn_markdown(
    user_prompt: Option<&StoredEvent>,
    events: &[&StoredEvent],
    pair_by_id: &HashMap<&str, &TimelineToolPair>,
) -> String {
    let mut parts = Vec::new();
    if let Some(prompt) = user_prompt {
        let prompt_text = user_prompt_text(prompt);
        if !prompt_text.trim().is_empty() {
            parts.push(format_prompt(&prompt_text));
        }
    }

    for event in events.iter().copied() {
        if user_prompt.is_some_and(|prompt| std::ptr::eq(prompt, event)) {
            continue;
        }
        if is_tool_result_event(event) {
            continue;
        }
        if is_assistant_event(event) {
            let formatted = format_assistant_event_markdown(event, pair_by_id);
            if !formatted.is_empty() {
                parts.push(formatted);
            }
        }
    }

    parts.join("\n\n")
}

fn format_assistant_event_markdown(
    event: &StoredEvent,
    pair_by_id: &HashMap<&str, &TimelineToolPair>,
) -> String {
    let mut parts = Vec::new();
    for block in &event.blocks {
        match block.kind {
            BlockKind::Text => {
                if let Some(text) = block
                    .text
                    .as_ref()
                    .map(|text| text.trim())
                    .filter(|text| !text.is_empty())
                {
                    parts.push(text.to_string());
                }
            }
            BlockKind::ToolUse => {
                let Some(tool_id) = block.tool_id.as_deref() else {
                    continue;
                };
                if let Some(pair) = pair_by_id.get(tool_id) {
                    parts.push(format_tool_pair_markdown(pair));
                }
            }
            _ => {}
        }
    }
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

fn format_tool_pair_markdown(pair: &TimelineToolPair) -> String {
    let header = format!(
        "**Tool:** `{}`{}{}",
        pair.operation_type.as_deref().unwrap_or(pair.name.as_str()),
        tool_one_line(pair),
        tool_status(pair)
    );
    let input = format_tool_input(pair);
    let result = format_tool_result(pair);
    [Some(header), input, result]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n")
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

fn format_tool_input(pair: &TimelineToolPair) -> Option<String> {
    let input = pair.input.as_ref()?;
    // Anything that canonicalised to `file_edits` renders the same —
    // tool-agnostic. Claude Edit / MultiEdit, codex apply_patch, and
    // any future tool all land here.
    if let Some(rendered) = format_file_edits_input(input) {
        return Some(rendered);
    }
    if let Some(command) = input
        .as_object()
        .and_then(|obj| obj.get("command").or_else(|| obj.get("cmd")))
        .and_then(Value::as_str)
        .filter(|command| !command.is_empty())
    {
        return Some(fence("bash", command.to_string()));
    }
    match pair_operation_type(pair) {
        "write" => format_write_input(input),
        "bash" | "exec" | "exec_command" => None,
        "todo_write" => {
            let todos = input
                .as_object()
                .and_then(|obj| obj.get("todos"))
                .and_then(Value::as_array)?;
            if todos.is_empty() {
                return None;
            }
            Some(
                todos
                    .iter()
                    .filter_map(|todo| todo.as_object())
                    .map(|todo| {
                        let status = match todo.get("status").and_then(Value::as_str) {
                            Some("completed") => "[x]",
                            Some("in_progress") => "[~]",
                            _ => "[ ]",
                        };
                        let content = todo
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        format!("- {status} {content}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }
        _ => Some(fence(
            "json",
            serde_json::to_string_pretty(input).unwrap_or_else(|_| "{}".to_string()),
        )),
    }
}

fn format_file_edits_input(input: &Value) -> Option<String> {
    let entries = input
        .as_object()
        .and_then(|obj| obj.get("file_edits"))
        .and_then(Value::as_array)?;
    if entries.is_empty() {
        return None;
    }
    let chunks: Vec<String> = entries.iter().filter_map(render_file_edit_entry).collect();
    if chunks.is_empty() {
        None
    } else {
        Some(chunks.join("\n\n---\n\n"))
    }
}

fn render_file_edit_entry(entry: &Value) -> Option<String> {
    let obj = entry.as_object()?;
    let path = obj
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("(no path)");
    let operation = obj
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("update");
    let old_path = obj.get("old_path").and_then(Value::as_str);

    let header = match (operation, old_path) {
        ("move", Some(from)) => format!("**{operation}**: `{from}` → `{path}`"),
        _ => format!("**{operation}**: `{path}`"),
    };

    let body = if let Some(in_out) = obj.get("in_out").and_then(Value::as_object) {
        // `in_out` form (Claude Edit / MultiEdit and anything else
        // that ships authoritative before/after strings): render as
        // a reconstructed unified diff for the markdown surface.
        let old_text = in_out
            .get("old_text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let new_text = in_out
            .get("new_text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if old_text.is_empty() && new_text.is_empty() {
            return Some(header);
        }
        fence("diff", unified_diff(old_text, new_text))
    } else if let Some(diff) = obj.get("diff").and_then(Value::as_str) {
        // `diff` form (codex apply_patch): pass the patch text
        // through verbatim, fenced as a diff.
        if diff.trim().is_empty() {
            return Some(header);
        }
        fence("diff", diff.to_string())
    } else {
        return Some(header);
    };

    Some(format!("{header}\n\n{body}"))
}

fn format_write_input(input: &Value) -> Option<String> {
    let obj = input.as_object()?;
    let content = obj
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if content.is_empty() {
        None
    } else {
        Some(fence("", content.to_string()))
    }
}

fn format_tool_result(pair: &TimelineToolPair) -> Option<String> {
    let result = pair.result.as_ref()?;
    let body = result.content.as_deref().unwrap_or_default();
    if body.is_empty() {
        return None;
    }
    let label = if pair.is_error {
        "Result (error)"
    } else {
        "Result"
    };
    Some(format!("_{label}_\n\n{}", fence("", body.to_string())))
}

fn unified_diff(old_text: &str, new_text: &str) -> String {
    old_text
        .lines()
        .map(|line| format!("- {line}"))
        .chain(new_text.lines().map(|line| format!("+ {line}")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn fence(lang: &str, body: String) -> String {
    let fence = if body.contains("```") { "````" } else { "```" };
    format!("{fence}{lang}\n{body}\n{fence}")
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
    use super::truncate;

    #[test]
    fn truncate_handles_multibyte_boundaries() {
        let text = "a".repeat(1498) + "—tail";
        let truncated = truncate(&text, 1500);
        assert!(truncated.ends_with('…'));
        assert!(truncated.is_char_boundary(truncated.len()));
    }
}
