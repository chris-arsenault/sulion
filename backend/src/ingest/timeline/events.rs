//! How the timeline reads one canonical event: who spoke, whether it is a
//! prompt, a tool call or result, or bookkeeping, and its display text.

use serde_json::Value;

use crate::ingest::canonical::{BlockKind, OperationCategory};

use super::render::truncate;
use super::{is_bookkeeping_record, record_speaker, StoredEvent};

#[derive(Debug, Clone)]
pub(crate) struct ToolUseView {
    pub(crate) id: Option<String>,
    pub(crate) name: String,
    pub(crate) raw_name: Option<String>,
    pub(crate) operation_type: Option<String>,
    pub(crate) category: Option<OperationCategory>,
    pub(crate) input: Option<Value>,
    pub(crate) is_error: bool,
}

pub(crate) fn tool_uses_in(event: &StoredEvent) -> Vec<ToolUseView> {
    event
        .blocks
        .iter()
        .filter(|block| block.kind == BlockKind::ToolUse)
        .map(|block| ToolUseView {
            is_error: block.is_error.unwrap_or(false),
            id: block.tool_id.clone(),
            name: block
                .tool_name_canonical
                .clone()
                .or_else(|| block.tool_name.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            raw_name: block.tool_name.clone(),
            operation_type: block.operation_type.clone(),
            category: block.operation_category,
            input: block.tool_input.clone(),
        })
        .collect()
}

pub(crate) fn text_blocks_in(event: &StoredEvent) -> Vec<String> {
    event
        .blocks
        .iter()
        .filter(|block| block.kind == BlockKind::Text)
        .filter_map(|block| block.text.clone())
        .collect()
}

fn thinking_texts_in(event: &StoredEvent) -> Vec<String> {
    event
        .blocks
        .iter()
        .filter(|block| block.kind == BlockKind::Thinking)
        .filter_map(|block| block.text.as_ref().map(|text| text.trim().to_string()))
        .filter(|text| !text.is_empty())
        .collect()
}

pub(crate) fn has_useful_thinking(event: &StoredEvent) -> bool {
    !thinking_texts_in(event).is_empty()
}

pub(crate) fn is_tool_result_event(event: &StoredEvent) -> bool {
    event
        .blocks
        .iter()
        .any(|block| block.kind == BlockKind::ToolResult)
}

pub(crate) fn is_real_user_prompt(event: &StoredEvent) -> bool {
    event_speaker(event) == "user"
        && !is_bookkeeping_event(event)
        && !is_tool_result_event(event)
        && !is_claude_task_notification(event)
}

fn is_claude_task_notification(event: &StoredEvent) -> bool {
    event.agent == "claude-code"
        && event.blocks.iter().any(|block| {
            if block.kind != BlockKind::Text {
                return false;
            }
            let Some(text) = block.text.as_deref() else {
                return false;
            };
            let text = text.trim();
            text.starts_with("<task-notification>")
                && text.ends_with("</task-notification>")
                && text.contains("<task-id>")
        })
}

pub(crate) fn is_assistant_event(event: &StoredEvent) -> bool {
    event_speaker(event) == "assistant"
}

pub(crate) fn is_summary_event(event: &StoredEvent) -> bool {
    event_speaker(event) == "summary"
}

pub(crate) fn is_system_event(event: &StoredEvent) -> bool {
    event_speaker(event) == "system"
}

pub(crate) fn is_bookkeeping_event(event: &StoredEvent) -> bool {
    is_bookkeeping_record(
        &event.kind,
        event.speaker.as_deref(),
        event.subtype.as_deref(),
        event.is_meta,
        &event.blocks,
    )
}

pub(crate) fn advances_turn_time(event: &StoredEvent) -> bool {
    // Completion telemetry is hidden bookkeeping, but still ends actual work.
    matches!(
        event.subtype.as_deref(),
        Some("turn_duration" | "stop_hook_summary")
    ) || !is_bookkeeping_event(event)
}

pub(crate) fn event_speaker(event: &StoredEvent) -> &str {
    record_speaker(&event.kind, event.speaker.as_deref())
}

pub(crate) fn user_prompt_text(event: &StoredEvent) -> String {
    text_blocks_in(event).join(" ")
}

pub(crate) fn first_paragraph(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let paragraphs: Vec<&str> = trimmed.split("\n\n").collect();
    let first = paragraphs
        .first()
        .copied()
        .unwrap_or(trimmed)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let has_more = paragraphs
        .iter()
        .skip(1)
        .any(|part| !part.trim().is_empty());
    if first.chars().count() <= max {
        if has_more {
            format!("{first} …")
        } else {
            first
        }
    } else {
        truncate(&first, max)
    }
}
