use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::ingest::canonical::{BlockKind, OperationCategory};

use super::render::{format_turn_markdown, pair_operation_type, subagent_title};
use super::{
    ProjectionFilters, SpeakerFacet, StoredEvent, TimelineAssistantItem, TimelineChunk,
    TimelineGenericDetails, TimelineResponse, TimelineSubagent, TimelineToolPair,
    TimelineToolResult, TimelineTurn, BOOKKEEPING_KINDS,
};

pub fn project_timeline(
    events: &[StoredEvent],
    total_event_count: i64,
    filters: &ProjectionFilters,
) -> TimelineResponse {
    let filtered_events: Vec<&StoredEvent> = events
        .iter()
        .filter(|event| filters.show_bookkeeping || !is_bookkeeping_event(event))
        .filter(|event| filters.show_sidechain || !event.is_sidechain)
        .collect();

    let turns = project_turns(&filtered_events, events, filters, true)
        .into_iter()
        .filter(|turn| turn_matches_filters(turn, filters))
        .collect();

    TimelineResponse {
        session_uuid: None,
        session_agent: None,
        total_event_count,
        turns,
    }
}

fn project_turns(
    events: &[&StoredEvent],
    all_events: &[StoredEvent],
    filters: &ProjectionFilters,
    include_subagent_links: bool,
) -> Vec<TimelineTurn> {
    group_into_turns(events)
        .into_iter()
        .map(|turn| project_turn(turn, all_events, filters, include_subagent_links))
        .collect()
}

pub(crate) struct TurnSeed<'a> {
    pub(crate) id: i64,
    pub(crate) user_prompt: Option<&'a StoredEvent>,
    pub(crate) events: Vec<&'a StoredEvent>,
    pub(crate) start_timestamp: DateTime<Utc>,
    pub(crate) end_timestamp: DateTime<Utc>,
    pub(crate) duration_ms: i64,
}

pub(crate) fn group_into_turns<'a>(events: &[&'a StoredEvent]) -> Vec<TurnSeed<'a>> {
    let mut turns = Vec::new();
    let mut current_idx: Option<usize> = None;

    for event in events.iter().copied() {
        if is_real_user_prompt(event) {
            turns.push(new_turn(Some(event), None));
            current_idx = Some(turns.len() - 1);
            continue;
        }

        if current_idx.is_none() {
            turns.push(new_turn(None, Some(event)));
            current_idx = Some(turns.len() - 1);
        }

        let turn = &mut turns[current_idx.expect("turn exists")];
        turn.events.push(event);
        turn.end_timestamp = event.timestamp;
        turn.duration_ms = duration_ms_between(turn.start_timestamp, turn.end_timestamp);
    }

    turns
}

fn new_turn<'a>(prompt: Option<&'a StoredEvent>, seed: Option<&'a StoredEvent>) -> TurnSeed<'a> {
    let first = prompt.or(seed).expect("turn needs prompt or seed");
    TurnSeed {
        id: first.byte_offset,
        user_prompt: prompt,
        events: prompt.into_iter().collect(),
        start_timestamp: first.timestamp,
        end_timestamp: first.timestamp,
        duration_ms: 0,
    }
}

pub(crate) fn project_turn(
    turn: TurnSeed<'_>,
    all_events: &[StoredEvent],
    filters: &ProjectionFilters,
    include_subagent_links: bool,
) -> TimelineTurn {
    let mut tool_pairs = Vec::new();
    let mut results: HashMap<String, (ToolResultView, &StoredEvent)> = HashMap::new();
    let mut use_order = 0_usize;
    let mut ordered_uses = Vec::new();
    let mut thinking_count = 0_usize;
    let mut has_errors = false;

    for event in turn.events.iter().copied() {
        if is_assistant_event(event) {
            for tool in tool_uses_in(event) {
                let id = tool
                    .id
                    .clone()
                    .unwrap_or_else(|| format!("noid-{use_order}"));
                ordered_uses.push((id, tool, event));
                use_order += 1;
            }
            if has_useful_thinking(event) {
                thinking_count += 1;
            }
        }

        if is_tool_result_event(event) {
            for result in tool_results_in(event) {
                let id = result
                    .tool_use_id
                    .clone()
                    .unwrap_or_else(|| format!("noid-{use_order}"));
                if result.is_error {
                    has_errors = true;
                }
                results.insert(id, (result, event));
            }
        }
    }

    for (id, tool, event) in ordered_uses {
        let result_match = results.get(&id).cloned();

        let mut pair = TimelineToolPair {
            id: id.clone(),
            name: tool.name,
            raw_name: tool.raw_name,
            operation_type: tool.operation_type,
            category: tool.category,
            input: tool.input,
            result: result_match.as_ref().map(|(result, _)| TimelineToolResult {
                content: result.content.clone(),
                payload: result.payload.clone(),
                is_error: result.is_error,
            }),
            is_error: result_match
                .as_ref()
                .map(|(result, _)| result.is_error)
                .unwrap_or(false),
            is_pending: result_match.is_none(),
            file_touches: Vec::new(),
            subagent: None,
        };

        if include_subagent_links && pair_operation_type(&pair) == "task" && !pair.id.is_empty() {
            pair.subagent =
                project_subagent(all_events, &pair, event.event_uuid.as_deref()).map(Box::new);
        }

        tool_pairs.push(pair);
    }

    let pair_by_id: HashMap<&str, &TimelineToolPair> = tool_pairs
        .iter()
        .map(|pair| (pair.id.as_str(), pair))
        .collect();
    let markdown = format_turn_markdown(turn.user_prompt, &turn.events, &pair_by_id);
    let chunks = build_chunks(turn.user_prompt, &turn.events, &pair_by_id, filters);

    TimelineTurn {
        id: turn.id,
        turn_key: None,
        preview: turn_preview(turn.user_prompt, &turn.events),
        user_prompt_text: turn.user_prompt.map(user_prompt_text),
        start_timestamp: turn.start_timestamp,
        end_timestamp: turn.end_timestamp,
        duration_ms: turn.duration_ms,
        event_count: turn.events.len(),
        operation_count: tool_pairs.len(),
        tool_pairs,
        thinking_count,
        has_errors,
        markdown,
        chunks,
        pty_session_id: None,
        session_uuid: None,
        session_agent: None,
        session_label: None,
        session_state: None,
    }
}

fn build_chunks(
    user_prompt: Option<&StoredEvent>,
    events: &[&StoredEvent],
    pair_by_id: &HashMap<&str, &TimelineToolPair>,
    filters: &ProjectionFilters,
) -> Vec<TimelineChunk> {
    #[derive(Default)]
    struct PendingAssistant {
        items: Vec<TimelineAssistantItem>,
        thinking: Vec<String>,
        has_text: bool,
    }

    let mut chunks = Vec::new();
    let mut pending = PendingAssistant::default();

    let flush_pending = |chunks: &mut Vec<TimelineChunk>, pending: &mut PendingAssistant| {
        if pending.has_text || !pending.thinking.is_empty() {
            chunks.push(TimelineChunk::Assistant {
                items: std::mem::take(&mut pending.items),
                thinking: std::mem::take(&mut pending.thinking),
            });
        } else {
            pending.items.clear();
            pending.thinking.clear();
        }
        pending.has_text = false;
    };

    for event in events.iter().copied() {
        if user_prompt.is_some_and(|prompt| std::ptr::eq(prompt, event)) {
            continue;
        }
        if is_tool_result_event(event) {
            continue;
        }
        if !event_is_visible(event, filters) {
            continue;
        }

        if is_assistant_event(event) {
            let mut visible_pairs = Vec::new();
            for block in &event.blocks {
                match block.kind {
                    BlockKind::Text => {
                        if let Some(text) = block.text.as_ref().filter(|text| !text.is_empty()) {
                            pending
                                .items
                                .push(TimelineAssistantItem::Text { text: text.clone() });
                            pending.has_text = true;
                        }
                    }
                    BlockKind::Thinking => {
                        if let Some(text) = block
                            .text
                            .as_ref()
                            .map(|text| text.trim())
                            .filter(|text| !text.is_empty())
                        {
                            pending.thinking.push(text.to_string());
                        }
                    }
                    BlockKind::ToolUse => {
                        let Some(pair_id) = block.tool_id.as_deref() else {
                            continue;
                        };
                        let Some(pair) = pair_by_id.get(pair_id) else {
                            continue;
                        };
                        if !tool_pair_is_visible(pair, filters) {
                            continue;
                        }
                        pending.items.push(TimelineAssistantItem::Tool {
                            pair_id: pair_id.to_string(),
                        });
                        visible_pairs.push(pair_id.to_string());
                    }
                    _ => {}
                }
            }

            if !visible_pairs.is_empty() {
                flush_pending(&mut chunks, &mut pending);
                for pair_id in visible_pairs {
                    chunks.push(TimelineChunk::Tool { pair_id });
                }
            }
            continue;
        }

        flush_pending(&mut chunks, &mut pending);
        if is_summary_event(event) {
            chunks.push(TimelineChunk::Summary {
                subtype: event.subtype.clone(),
                text: text_blocks_in(event).join(" "),
            });
        } else if is_system_event(event) {
            chunks.push(TimelineChunk::System {
                subtype: event.subtype.clone(),
                text: text_blocks_in(event).join(" "),
                is_meta: event.is_meta,
            });
        } else {
            chunks.push(TimelineChunk::Generic {
                label: event.kind.clone(),
                details: TimelineGenericDetails {
                    event_uuid: event.event_uuid.clone(),
                    parent_event_uuid: event.parent_event_uuid.clone(),
                    related_tool_use_id: event.related_tool_use_id.clone(),
                    subtype: event.subtype.clone(),
                    speaker: event.speaker.clone(),
                    content_kind: event.content_kind.clone(),
                    blocks: event.blocks.clone(),
                },
            });
        }
    }

    flush_pending(&mut chunks, &mut pending);
    chunks
}

fn project_subagent(
    all_events: &[StoredEvent],
    pair: &TimelineToolPair,
    seed_uuid: Option<&str>,
) -> Option<TimelineSubagent> {
    let selected = collect_subagent_events(all_events, &pair.id, seed_uuid);
    if selected.is_empty() {
        return None;
    }

    let turns = project_turns(&selected, all_events, &ProjectionFilters::default(), false);
    Some(TimelineSubagent {
        title: subagent_title(pair),
        event_count: selected.len(),
        turns,
    })
}

fn collect_subagent_events<'a>(
    events: &'a [StoredEvent],
    tool_use_id: &str,
    seed_uuid: Option<&str>,
) -> Vec<&'a StoredEvent> {
    let mut uuids_in_lineage = HashSet::new();
    if let Some(seed_uuid) = seed_uuid {
        uuids_in_lineage.insert(seed_uuid.to_string());
    }

    for event in events {
        if event.related_tool_use_id.as_deref() == Some(tool_use_id) {
            if let Some(uuid) = &event.event_uuid {
                uuids_in_lineage.insert(uuid.clone());
            }
        }
    }

    let mut added = true;
    while added {
        added = false;
        for event in events {
            if !event.is_sidechain {
                continue;
            }
            let Some(uuid) = &event.event_uuid else {
                continue;
            };
            if uuids_in_lineage.contains(uuid) {
                continue;
            }
            if let Some(parent) = &event.parent_event_uuid {
                if uuids_in_lineage.contains(parent) {
                    uuids_in_lineage.insert(uuid.clone());
                    added = true;
                }
            }
        }
    }

    events
        .iter()
        .filter(|event| {
            (event.is_sidechain
                && event
                    .event_uuid
                    .as_ref()
                    .map(|uuid| uuids_in_lineage.contains(uuid))
                    .unwrap_or(false))
                || event.related_tool_use_id.as_deref() == Some(tool_use_id)
        })
        .collect()
}

fn turn_matches_filters(turn: &TimelineTurn, filters: &ProjectionFilters) -> bool {
    if filters.errors_only && !turn.has_errors {
        return false;
    }

    if !filters.file_path.trim().is_empty() {
        let needle = filters.file_path.to_lowercase();
        if !turn
            .tool_pairs
            .iter()
            .any(|pair| tool_pair_matches_file_path(pair, &needle))
        {
            return false;
        }
    }

    true
}

fn tool_pair_matches_file_path(pair: &TimelineToolPair, needle: &str) -> bool {
    let Some(Value::Object(input)) = &pair.input else {
        return false;
    };

    ["path", "pattern", "command", "query", "url"]
        .iter()
        .filter_map(|key| input.get(*key))
        .filter_map(Value::as_str)
        .any(|value| value.to_lowercase().contains(needle))
}

fn event_is_visible(event: &StoredEvent, filters: &ProjectionFilters) -> bool {
    let Some(speaker) = speaker_facet_of(event) else {
        return true;
    };
    !filters.hidden_speakers.contains(&speaker)
}

fn tool_pair_is_visible(pair: &TimelineToolPair, filters: &ProjectionFilters) -> bool {
    pair.category
        .map(|category| !filters.hidden_operation_categories.contains(&category))
        .unwrap_or(true)
}

#[derive(Debug, Clone)]
struct ToolUseView {
    id: Option<String>,
    name: String,
    raw_name: Option<String>,
    operation_type: Option<String>,
    category: Option<OperationCategory>,
    input: Option<Value>,
}

#[derive(Debug, Clone)]
struct ToolResultView {
    tool_use_id: Option<String>,
    content: Option<String>,
    payload: Option<Value>,
    is_error: bool,
}

fn tool_uses_in(event: &StoredEvent) -> Vec<ToolUseView> {
    event
        .blocks
        .iter()
        .filter(|block| block.kind == BlockKind::ToolUse)
        .map(|block| ToolUseView {
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

fn tool_results_in(event: &StoredEvent) -> Vec<ToolResultView> {
    event
        .blocks
        .iter()
        .filter(|block| block.kind == BlockKind::ToolResult)
        .map(|block| ToolResultView {
            tool_use_id: block.tool_id.clone(),
            content: block.text.clone(),
            payload: block.tool_output.clone(),
            is_error: block.is_error.unwrap_or(false),
        })
        .collect()
}

fn text_blocks_in(event: &StoredEvent) -> Vec<String> {
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

fn has_useful_thinking(event: &StoredEvent) -> bool {
    !thinking_texts_in(event).is_empty()
}

pub(crate) fn is_tool_result_event(event: &StoredEvent) -> bool {
    event
        .blocks
        .iter()
        .any(|block| block.kind == BlockKind::ToolResult)
}

fn is_real_user_prompt(event: &StoredEvent) -> bool {
    event_speaker(event) == "user" && !is_tool_result_event(event)
}

pub(crate) fn is_assistant_event(event: &StoredEvent) -> bool {
    event_speaker(event) == "assistant"
}

fn is_summary_event(event: &StoredEvent) -> bool {
    event_speaker(event) == "summary"
}

fn is_system_event(event: &StoredEvent) -> bool {
    event_speaker(event) == "system"
}

fn is_bookkeeping_event(event: &StoredEvent) -> bool {
    BOOKKEEPING_KINDS.contains(&event.kind.as_str()) || (is_system_event(event) && event.is_meta)
}

fn speaker_facet_of(event: &StoredEvent) -> Option<SpeakerFacet> {
    let speaker = event_speaker(event);
    if speaker == "assistant" {
        Some(SpeakerFacet::Assistant)
    } else if is_tool_result_event(event) {
        Some(SpeakerFacet::ToolResult)
    } else if speaker == "user" {
        Some(SpeakerFacet::User)
    } else {
        None
    }
}

fn event_speaker(event: &StoredEvent) -> &str {
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

fn turn_preview(user_prompt: Option<&StoredEvent>, events: &[&StoredEvent]) -> String {
    if let Some(prompt) = user_prompt {
        let text = user_prompt_text(prompt);
        if !text.is_empty() {
            return first_paragraph(&text, 280);
        }
    }

    if let Some(first_assistant) = events
        .iter()
        .copied()
        .find(|event| is_assistant_event(event))
    {
        let text = text_blocks_in(first_assistant).join(" ");
        if !text.is_empty() {
            return format!("(assistant) {}", first_paragraph(&text, 260));
        }
    }

    "(no user prompt)".to_string()
}

fn first_paragraph(text: &str, max: usize) -> String {
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
    if first.len() <= max {
        if has_more {
            format!("{first} …")
        } else {
            first
        }
    } else {
        format!("{}…", &first[..max.saturating_sub(1)])
    }
}

fn duration_ms_between(a: DateTime<Utc>, b: DateTime<Utc>) -> i64 {
    (b - a).num_milliseconds().max(0)
}
