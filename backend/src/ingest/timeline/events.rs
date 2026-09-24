//! How the timeline reads one canonical event: who spoke, whether it is a
//! prompt, a tool call or result, or bookkeeping, and its display text.

use serde_json::Value;

use crate::ingest::canonical::{BlockKind, OperationCategory};

use super::render::truncate;
use super::{is_bookkeeping_system_subtype, StoredEvent, BOOKKEEPING_KINDS};

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
        && !is_tool_result_event(event)
        && !is_claude_task_notification(event)
        && !is_local_command_event(event)
}

/// Local slash-command plumbing (`/model`, `/login`, …) arrives as user
/// records; it must not seed turns or read as a prompt.
pub(crate) fn is_local_command_event(event: &StoredEvent) -> bool {
    event_speaker(event) == "user"
        && event
            .blocks
            .iter()
            .find(|block| block.kind == BlockKind::Text)
            .and_then(|block| block.text.as_deref())
            .is_some_and(super::is_local_command_text)
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
    // is_meta covers any speaker: claude meta-system records and codex
    // plumbing records (world_state, turn_context, …) alike.
    (BOOKKEEPING_KINDS.contains(&event.kind.as_str())
        && event.subtype.as_deref() != Some("queued_user_prompt"))
        || event.is_meta
        || (is_system_event(event) && is_bookkeeping_system_subtype(event.subtype.as_deref()))
        || is_local_command_event(event)
}

pub(crate) fn event_speaker(event: &StoredEvent) -> &str {
    if let Some(speaker) = &event.speaker {
        return speaker;
    }
    match event.kind.as_str() {
        "assistant" => "assistant",
        "user" => "user",
        "system" => "system",
        "summary" => "summary",
        _ => "other",
    }
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
