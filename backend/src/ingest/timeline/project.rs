use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::ingest::canonical::{BlockKind, OperationCategory};

use super::render::{format_turn_markdown, pair_operation_type, subagent_title, truncate};
use super::{
    is_bookkeeping_system_subtype, ProjectionFilters, SpeakerFacet, StoredEvent,
    TimelineAssistantItem, TimelineChunk, TimelineGenericDetails, TimelineResponse,
    TimelineSubagent, TimelineToolPair, TimelineToolResult, TimelineTurn, BOOKKEEPING_KINDS,
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

    let turns = project_turns(&filtered_events, events, filters, SUBAGENT_LINK_DEPTH)
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

/// How many levels of Task pairs get an embedded subagent projection.
/// 2 = top-level turns link their subagents, and those subagent turns link
/// their own nested Tasks; deeper nesting renders as plain tool pairs.
pub(crate) const SUBAGENT_LINK_DEPTH: u8 = 2;

fn project_turns(
    events: &[&StoredEvent],
    all_events: &[StoredEvent],
    filters: &ProjectionFilters,
    subagent_link_depth: u8,
) -> Vec<TimelineTurn> {
    group_into_turns(events)
        .into_iter()
        .map(|turn| project_turn(turn, all_events, filters, subagent_link_depth))
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
    // Main-line turn currently receiving events. Sidechain events never
    // touch it: concurrent subagents interleave with the parent's own
    // work by timestamp, and letting a sidechain seed capture the main
    // pointer swallowed every later main-turn event.
    let mut current_main: Option<usize> = None;
    // One open sidechain turn per origin: merged descendant sessions
    // are keyed by their session uuid, the parent's own in-file
    // sidechain records by None. Two concurrent subagents must land in
    // two turns even though their events interleave.
    let mut current_sidechain: HashMap<Option<Uuid>, usize> = HashMap::new();
    // Bookkeeping that arrives before the first real turn. Claude
    // stamps attachment records a few ms before their prompt, so
    // letting them seed a turn created a decoy orphan the incremental
    // rebuild then attached all real work to. They wait here and join
    // the first turn that actually forms.
    let mut pending_prefix: Vec<&'a StoredEvent> = Vec::new();

    for event in events.iter().copied() {
        let sidechain_key =
            (event.is_sidechain || event.source_session.is_some()).then_some(event.source_session);

        let target = match sidechain_key {
            Some(key) => {
                if is_real_user_prompt(event) {
                    turns.push(new_turn(Some(event), None));
                    let idx = turns.len() - 1;
                    current_sidechain.insert(key, idx);
                    continue;
                }
                match current_sidechain.get(&key) {
                    Some(idx) => *idx,
                    None => {
                        turns.push(new_turn(None, Some(event)));
                        let idx = turns.len() - 1;
                        current_sidechain.insert(key, idx);
                        idx
                    }
                }
            }
            None => {
                if is_real_user_prompt(event) {
                    turns.push(new_turn(Some(event), None));
                    let idx = turns.len() - 1;
                    current_main = Some(idx);
                    for pending in pending_prefix.drain(..) {
                        turns[idx].events.push(pending);
                    }
                    continue;
                }
                match current_main {
                    Some(idx) => idx,
                    None => {
                        if is_bookkeeping_event(event) {
                            pending_prefix.push(event);
                            continue;
                        }
                        turns.push(new_turn(None, Some(event)));
                        let idx = turns.len() - 1;
                        current_main = Some(idx);
                        for pending in pending_prefix.drain(..) {
                            turns[idx].events.push(pending);
                        }
                        idx
                    }
                }
            }
        };

        let turn = &mut turns[target];
        turn.events.push(event);
        turn.end_timestamp = event.timestamp;
        turn.duration_ms = duration_ms_between(turn.start_timestamp, turn.end_timestamp);
    }

    // A session of nothing but bookkeeping still needs a home.
    if !pending_prefix.is_empty() && turns.is_empty() {
        turns.push(new_turn(None, Some(pending_prefix[0])));
        let turn = &mut turns[0];
        for pending in pending_prefix.drain(..) {
            turn.events.push(pending);
        }
    }

    turns
}

fn new_turn<'a>(prompt: Option<&'a StoredEvent>, seed: Option<&'a StoredEvent>) -> TurnSeed<'a> {
    let first = prompt.or(seed).expect("turn needs prompt or seed");
    TurnSeed {
        id: turn_seed_id(first),
        user_prompt: prompt,
        events: prompt.into_iter().collect(),
        start_timestamp: first.timestamp,
        end_timestamp: first.timestamp,
        duration_ms: 0,
    }
}

/// Stable turn identity. Root-session turns keep the seed event's byte
/// offset (existing ids must survive rebuilds). Turns seeded by merged
/// descendant events fold the origin session into the id: subagent
/// transcripts all start at offset 0, and `timeline_turns` upserts on
/// `(session_uuid, turn_id)`, so plain offsets made two concurrent
/// subagents clobber each other's turn.
fn turn_seed_id(seed: &StoredEvent) -> i64 {
    match seed.source_session {
        None => seed.byte_offset,
        Some(session) => {
            let high = u32::from_be_bytes(
                session.as_bytes()[..4]
                    .try_into()
                    .expect("uuid has 4 bytes"),
            ) as i64;
            ((high & 0x3fff_ffff) << 32) | (seed.byte_offset & 0xffff_ffff)
        }
    }
}

pub(crate) fn project_turn(
    turn: TurnSeed<'_>,
    all_events: &[StoredEvent],
    filters: &ProjectionFilters,
    subagent_link_depth: u8,
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

        if subagent_link_depth > 0 && pair_operation_type(&pair) == "task" && !pair.id.is_empty() {
            pair.subagent = project_subagent(
                all_events,
                &pair,
                event.event_uuid.as_deref(),
                subagent_link_depth - 1,
            )
            .map(Box::new);
        }

        tool_pairs.push(pair);
    }

    let pair_by_id: HashMap<&str, &TimelineToolPair> = tool_pairs
        .iter()
        .map(|pair| (pair.id.as_str(), pair))
        .collect();
    let markdown = format_turn_markdown(turn.user_prompt, &turn.events, &pair_by_id);
    let chunks = build_chunks(turn.user_prompt, &turn.events, &pair_by_id, filters);
    let is_sidechain = turn
        .user_prompt
        .map(|event| event.is_sidechain)
        .or_else(|| turn.events.first().copied().map(|event| event.is_sidechain))
        .unwrap_or(false);
    let (input_tokens, output_tokens) = turn_token_usage(&turn, all_events);

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
        is_sidechain,
        input_tokens,
        output_tokens,
        markdown,
        chunks,
        pty_session_id: None,
        session_uuid: None,
        session_agent: None,
        session_label: None,
        session_state: None,
    }
}

/// Tokens this turn consumed. Claude records per-message usage on
/// assistant events (streamed repeats dedupe by message id). Codex
/// records cumulative session totals; the turn's share is the clamped
/// difference between the last totals inside the turn and the last
/// totals before it.
fn turn_token_usage(turn: &TurnSeed<'_>, all_events: &[StoredEvent]) -> (i64, i64) {
    fn token(usage: &Value, key: &str) -> i64 {
        usage.get(key).and_then(Value::as_i64).unwrap_or(0)
    }

    let mut input = 0i64;
    let mut output = 0i64;
    let mut seen_messages: HashSet<&str> = HashSet::new();
    let mut codex_last: Option<&Value> = None;

    for event in turn.events.iter().copied() {
        let Some(usage) = event.usage_json.as_ref() else {
            continue;
        };
        if event.agent == "codex" {
            codex_last = Some(usage);
            continue;
        }
        if let Some(message_id) = event.usage_message_id.as_deref() {
            if !seen_messages.insert(message_id) {
                continue;
            }
        }
        input += token(usage, "input_tokens")
            + token(usage, "cache_read_input_tokens")
            + token(usage, "cache_creation_input_tokens");
        output += token(usage, "output_tokens");
    }

    if let Some(last) = codex_last {
        let first_offset = turn
            .events
            .first()
            .map(|event| event.byte_offset)
            .unwrap_or(i64::MIN);
        let baseline = all_events
            .iter()
            .take_while(|event| event.byte_offset < first_offset)
            .filter(|event| event.agent == "codex")
            .filter_map(|event| event.usage_json.as_ref())
            .last();
        let delta =
            |key: &str| (token(last, key) - baseline.map_or(0, |usage| token(usage, key))).max(0);
        input += delta("input_tokens");
        output += delta("output_tokens");
    }

    (input, output)
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
    link_depth: u8,
) -> Option<TimelineSubagent> {
    let selected = collect_subagent_events(all_events, &pair.id, seed_uuid);
    if selected.is_empty() {
        return None;
    }

    let turns = project_turns(
        &selected,
        all_events,
        &ProjectionFilters::default(),
        link_depth,
    );
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

fn is_summary_event(event: &StoredEvent) -> bool {
    event_speaker(event) == "summary"
}

fn is_system_event(event: &StoredEvent) -> bool {
    event_speaker(event) == "system"
}

fn is_bookkeeping_event(event: &StoredEvent) -> bool {
    // is_meta covers any speaker: claude meta-system records and codex
    // plumbing records (world_state, turn_context, …) alike.
    BOOKKEEPING_KINDS.contains(&event.kind.as_str())
        || event.is_meta
        || (is_system_event(event) && is_bookkeeping_system_subtype(event.subtype.as_deref()))
        || is_local_command_event(event)
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

fn duration_ms_between(a: DateTime<Utc>, b: DateTime<Utc>) -> i64 {
    (b - a).num_milliseconds().max(0)
}
