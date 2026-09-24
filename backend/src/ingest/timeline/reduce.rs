//! Incremental timeline reducer.
//!
//! Consumes one session's canonical events in byte-offset order. Each event
//! lands in one turn: it bumps that turn's counters, appends the event's
//! visible item, inserts an operation per tool call, and updates the
//! operation a result or runtime evidence belongs to. Nothing else is
//! rewritten; readers group items and compose digests. Rows it needs are
//! loaded on demand through [`Backend`]; the caller persists the batch with
//! the new cursor.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::ingest::canonical::{BlockKind, OperationCategory};

use super::events::{
    first_paragraph, has_useful_thinking, is_assistant_event, is_bookkeeping_event,
    is_real_user_prompt, is_summary_event, is_system_event, is_tool_result_event, text_blocks_in,
    tool_uses_in, user_prompt_text,
};
use super::file_touches::{extract_file_touches, FileTouchContext};
use super::runtime::{
    attach_to_input, attach_to_output, mark_uncorrelated, running_cell, runtime_evidence,
};
use super::{
    StoredEvent, TimelineAssistantItem, TimelineChunk, TimelineFileTouch, TimelineGenericDetails,
    TimelineToolPair, TimelineToolResult,
};

mod operations;

const NO_PROMPT_PREVIEW: &str = "(no user prompt)";

/// Per-session reducer state that is not a turn, item or operation.
#[derive(Debug, Clone)]
pub(crate) struct SessionState {
    /// Highest byte offset applied; the next batch starts after it.
    pub projected_through: i64,
    pub next_turn_ord: i32,
    /// Main-line turn receiving non-prompt events.
    pub current_main: Option<i64>,
    /// Sidechain turn receiving in-file sidechain events.
    pub current_sidechain: Option<i64>,
    /// Latest Codex cumulative totals, the baseline of the next turn.
    pub codex_input_total: Option<i64>,
    pub codex_output_total: Option<i64>,
    pub total_event_count: i64,
    pub turn_count: i64,
    pub latest_turn_id: Option<i64>,
    pub latest_event_at: Option<DateTime<Utc>>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            projected_through: -1,
            next_turn_ord: 0,
            current_main: None,
            current_sidechain: None,
            codex_input_total: None,
            codex_output_total: None,
            total_event_count: 0,
            turn_count: 0,
            latest_turn_id: None,
            latest_event_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TurnRow {
    pub turn_id: i64,
    pub turn_ord: i32,
    pub is_sidechain: bool,
    pub preview: String,
    pub user_prompt_text: Option<String>,
    pub prompt_event_uuid: Option<String>,
    pub start_timestamp: DateTime<Utc>,
    pub end_timestamp: DateTime<Utc>,
    pub duration_ms: i64,
    pub event_count: i32,
    pub operation_count: i32,
    pub thinking_count: i32,
    pub has_errors: bool,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// Codex cumulative totals when the turn began.
    pub usage_baseline_input: i64,
    pub usage_baseline_output: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpRow {
    pub turn_id: i64,
    pub operation_ord: i32,
    pub pair_id: String,
    pub name: String,
    pub raw_name: Option<String>,
    pub operation_type: Option<String>,
    pub category: Option<OperationCategory>,
    pub input: Option<Value>,
    pub result_content: Option<String>,
    pub result_payload: Option<Value>,
    pub result_is_error: bool,
    pub is_error: bool,
    pub is_pending: bool,
    pub call_offset: i64,
    pub call_at: DateTime<Utc>,
    /// Byte offset of the event that last changed the row.
    pub changed_at: i64,
    /// The call block's own error flag or failed runtime evidence.
    pub call_error: bool,
    /// Set while the call's result reports a still-running code cell.
    pub running_cell: Option<String>,
    /// When the call's work ended: its result, or the wait that completed
    /// its cell. Open calls enclose later runtime evidence.
    pub finished_at: Option<DateTime<Utc>>,
}

impl OpRow {
    pub(crate) fn key(&self) -> (i64, i32) {
        (self.turn_id, self.operation_ord)
    }

    pub(crate) fn pair(&self) -> TimelineToolPair {
        let result = if self.result_content.is_some() || self.result_payload.is_some() {
            Some(TimelineToolResult {
                content: self.result_content.clone(),
                payload: self.result_payload.clone(),
                is_error: self.result_is_error,
            })
        } else {
            None
        };
        TimelineToolPair {
            id: self.pair_id.clone(),
            name: self.name.clone(),
            raw_name: self.raw_name.clone(),
            operation_type: self.operation_type.clone(),
            category: self.category,
            input: self.input.clone(),
            result,
            is_error: self.is_error,
            is_pending: self.is_pending,
            file_touches: Vec::new(),
            subagent: None,
        }
    }

    fn tool_short_name(&self) -> Option<&str> {
        self.raw_name
            .as_deref()
            .and_then(|name| name.rsplit('.').next())
    }

    /// The runtime-evidence window of an `exec` call: evidence that started
    /// inside it, or arrived while it had not finished.
    fn encloses(&self, started: Option<DateTime<Utc>>) -> bool {
        if self.tool_short_name() != Some("exec") {
            return false;
        }
        match started {
            Some(start) => {
                start.timestamp_millis() >= self.call_at.timestamp_millis()
                    && self
                        .finished_at
                        .is_none_or(|end| start.timestamp_millis() <= end.timestamp_millis())
            }
            None => self.finished_at.is_none(),
        }
    }
}

/// A Claude response's latest usage and the turn it is charged to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MessageUsage {
    pub turn_id: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// A spawning call and the transcript it produced: a whole child session
/// (`child_turn_id` -1) or a sidechain turn of the same session.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ChildLink {
    pub session_uuid: Uuid,
    pub pair_id: String,
    pub child_session_uuid: Uuid,
    pub child_turn_id: i64,
}

/// Committed rows the reducer reads.
#[allow(async_fn_in_trait)]
pub(crate) trait Backend {
    async fn turn(&mut self, turn_id: i64) -> anyhow::Result<Option<TurnRow>>;
    /// Operations with this pair id whose call precedes `before`.
    async fn ops_by_pair(&mut self, pair_id: &str, before: i64) -> anyhow::Result<Vec<OpRow>>;
    async fn prompt_seen(&mut self, event_uuid: &str) -> anyhow::Result<bool>;
    async fn message_usage(&mut self, message_id: &str) -> anyhow::Result<Option<MessageUsage>>;
    /// `exec` calls before `before` that may enclose evidence started at
    /// `started` (a superset; the reducer applies the exact window).
    async fn exec_candidates(
        &mut self,
        before: i64,
        started: Option<DateTime<Utc>>,
    ) -> anyhow::Result<Vec<OpRow>>;
    async fn ops_by_running_cell(&mut self, cell: &str) -> anyhow::Result<Vec<OpRow>>;
}

/// Everything one batch changed, ready to persist.
#[derive(Debug, Default)]
pub(crate) struct Changes {
    pub state: SessionState,
    pub turns: Vec<TurnRow>,
    pub items: Vec<(i64, i64, TimelineChunk)>,
    pub new_ops: Vec<OpRow>,
    /// Existing operations updated, and whether their input changed.
    pub updated_ops: Vec<(OpRow, bool)>,
    /// Operations whose file touches are replaced, with the new touches.
    pub touches: Vec<(i64, i32, Vec<TimelineFileTouch>)>,
    pub usage: Vec<(String, MessageUsage)>,
    pub links: Vec<ChildLink>,
}

impl Changes {
    pub(crate) fn is_empty(&self) -> bool {
        self.turns.is_empty()
            && self.items.is_empty()
            && self.new_ops.is_empty()
            && self.updated_ops.is_empty()
            && self.links.is_empty()
    }
}

pub(crate) struct Reducer<B> {
    backend: B,
    session_uuid: Uuid,
    state: SessionState,
    /// Cursor when the batch began: rows at or before it are persisted.
    batch_start: i64,
    file_context: Option<FileTouchContext>,
    /// Set for a Claude subagent transcript: the parent session its events
    /// link back to.
    child_of: Option<Uuid>,
    turns: BTreeMap<i64, TurnRow>,
    dirty_turns: BTreeSet<i64>,
    items: Vec<(i64, i64, TimelineChunk)>,
    ops: BTreeMap<(i64, i32), OpRow>,
    new_ops: BTreeSet<(i64, i32)>,
    updated_ops: BTreeSet<(i64, i32)>,
    input_changed: BTreeSet<(i64, i32)>,
    ops_by_pair: HashMap<String, BTreeSet<(i64, i32)>>,
    queried_pairs: HashSet<String>,
    prompts: HashSet<String>,
    usage: HashMap<String, MessageUsage>,
    dirty_usage: BTreeSet<String>,
    links: BTreeSet<ChildLink>,
}

fn token(usage: &Value, key: &str) -> i64 {
    usage.get(key).and_then(Value::as_i64).unwrap_or(0)
}

impl<B: Backend> Reducer<B> {
    pub(crate) fn new(
        backend: B,
        session_uuid: Uuid,
        state: SessionState,
        file_context: Option<FileTouchContext>,
        child_of: Option<Uuid>,
    ) -> Self {
        Self {
            backend,
            session_uuid,
            batch_start: state.projected_through,
            state,
            file_context,
            child_of,
            turns: BTreeMap::new(),
            dirty_turns: BTreeSet::new(),
            items: Vec::new(),
            ops: BTreeMap::new(),
            new_ops: BTreeSet::new(),
            updated_ops: BTreeSet::new(),
            input_changed: BTreeSet::new(),
            ops_by_pair: HashMap::new(),
            queried_pairs: HashSet::new(),
            prompts: HashSet::new(),
            usage: HashMap::new(),
            dirty_usage: BTreeSet::new(),
            links: BTreeSet::new(),
        }
    }

    /// Apply one event. Events must arrive in byte-offset order.
    pub(crate) async fn apply(&mut self, event: StoredEvent) -> anyhow::Result<()> {
        self.state.projected_through = self.state.projected_through.max(event.byte_offset);
        if event.subtype.as_deref() == Some("inherited_history") {
            return Ok(());
        }
        let event = self.correlate_runtime(event).await?;
        self.record_links(&event);

        let is_prompt = is_real_user_prompt(&event);
        if is_prompt {
            if let Some(id) = &event.event_uuid {
                if self.prompts.contains(id) || self.backend.prompt_seen(id).await? {
                    return Ok(());
                }
                self.prompts.insert(id.clone());
            }
        }

        let turn_id = if event.is_sidechain {
            let current = if is_prompt {
                None
            } else {
                self.state.current_sidechain
            };
            match current {
                Some(turn_id) => turn_id,
                None => {
                    let turn_id = self.create_turn(&event, is_prompt, true);
                    self.state.current_sidechain = Some(turn_id);
                    self.link_sidechain_turn(turn_id, &event);
                    if is_prompt {
                        return Ok(());
                    }
                    turn_id
                }
            }
        } else if is_prompt {
            let turn_id = self.create_turn(&event, true, false);
            self.state.current_main = Some(turn_id);
            return Ok(());
        } else {
            match self.state.current_main {
                Some(turn_id) => turn_id,
                // Bookkeeping before the first turn belongs to no turn.
                None if is_bookkeeping_event(&event) => return Ok(()),
                None => {
                    let turn_id = self.create_turn(&event, false, false);
                    self.state.current_main = Some(turn_id);
                    turn_id
                }
            }
        };
        self.append_event(turn_id, &event).await
    }

    /// Hand the batch's changes to the caller for persistence.
    pub(crate) fn into_changes(self) -> (B, Changes) {
        let touches = self
            .new_ops
            .iter()
            .chain(&self.input_changed)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|key| self.ops.get(key))
            .map(|op| {
                let touches = extract_file_touches(&op.pair(), self.file_context.as_ref());
                (op.turn_id, op.operation_ord, touches)
            })
            .collect();
        let changes = Changes {
            turns: self
                .dirty_turns
                .iter()
                .filter_map(|id| self.turns.get(id).cloned())
                .collect(),
            items: self.items,
            new_ops: self
                .new_ops
                .iter()
                .filter_map(|key| self.ops.get(key).cloned())
                .collect(),
            updated_ops: self
                .updated_ops
                .iter()
                .filter(|key| !self.new_ops.contains(key))
                .filter_map(|key| {
                    let changed = self.input_changed.contains(key);
                    self.ops.get(key).map(|op| (op.clone(), changed))
                })
                .collect(),
            touches,
            usage: self
                .dirty_usage
                .iter()
                .filter_map(|id| self.usage.get(id).map(|usage| (id.clone(), usage.clone())))
                .collect(),
            links: self.links.into_iter().collect(),
            state: self.state,
        };
        (self.backend, changes)
    }

    // ── turns ───────────────────────────────────────────────────────────

    async fn load_turn(&mut self, turn_id: i64) -> anyhow::Result<bool> {
        if self.turns.contains_key(&turn_id) {
            return Ok(true);
        }
        match self.backend.turn(turn_id).await? {
            Some(row) => {
                self.turns.insert(turn_id, row);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn turn_mut(&mut self, turn_id: i64) -> &mut TurnRow {
        self.dirty_turns.insert(turn_id);
        self.turns
            .get_mut(&turn_id)
            .expect("turn loaded before it is changed")
    }

    /// A new turn seeded by `first`: its prompt, or the first event of a turn
    /// without one. A prompt is the turn's first event; any other seed is
    /// appended by the caller like every later event.
    fn create_turn(&mut self, first: &StoredEvent, prompt: bool, sidechain: bool) -> i64 {
        let turn_id = first.byte_offset;
        let prompt_text = prompt.then(|| user_prompt_text(first));
        let preview = prompt_text
            .as_deref()
            .filter(|text| !text.is_empty())
            .map_or_else(
                || NO_PROMPT_PREVIEW.to_string(),
                |text| first_paragraph(text, 280),
            );
        self.turns.insert(
            turn_id,
            TurnRow {
                turn_id,
                turn_ord: self.state.next_turn_ord,
                is_sidechain: sidechain,
                preview,
                user_prompt_text: prompt_text,
                prompt_event_uuid: first.event_uuid.clone().filter(|_| prompt),
                start_timestamp: first.timestamp,
                end_timestamp: first.timestamp,
                duration_ms: 0,
                event_count: i32::from(prompt),
                operation_count: 0,
                thinking_count: 0,
                has_errors: false,
                input_tokens: 0,
                output_tokens: 0,
                usage_baseline_input: self.state.codex_input_total.unwrap_or(0),
                usage_baseline_output: self.state.codex_output_total.unwrap_or(0),
            },
        );
        self.dirty_turns.insert(turn_id);
        self.state.next_turn_ord += 1;
        self.state.turn_count += 1;
        self.state.total_event_count += i64::from(prompt);
        self.state.latest_turn_id = self.state.latest_turn_id.max(Some(turn_id));
        self.state.latest_event_at = self.state.latest_event_at.max(Some(first.timestamp));
        turn_id
    }

    async fn append_event(&mut self, turn_id: i64, event: &StoredEvent) -> anyhow::Result<()> {
        if !self.load_turn(turn_id).await? {
            anyhow::bail!("turn {turn_id} of session {} is missing", self.session_uuid);
        }
        let turn = self.turn_mut(turn_id);
        turn.event_count += 1;
        turn.end_timestamp = event.timestamp;
        turn.duration_ms = (turn.end_timestamp - turn.start_timestamp)
            .num_milliseconds()
            .max(0);
        self.state.total_event_count += 1;
        self.state.latest_event_at = self.state.latest_event_at.max(Some(event.timestamp));

        if is_assistant_event(event) {
            self.apply_calls(turn_id, event).await?;
            let turn = self.turn_mut(turn_id);
            turn.thinking_count += i32::from(has_useful_thinking(event));
            if turn.preview == NO_PROMPT_PREVIEW {
                let text = text_blocks_in(event).join(" ");
                if !text.is_empty() {
                    turn.preview = format!("(assistant) {}", first_paragraph(&text, 260));
                }
            }
        }
        if is_tool_result_event(event) {
            self.apply_results(turn_id, event).await?;
        }
        if let Some(item) = visible_item(event) {
            self.items.push((turn_id, event.byte_offset, item));
        }
        self.apply_usage(turn_id, event).await
    }

    // ── children ────────────────────────────────────────────────────────

    /// Child transcripts are referenced, never copied. A Claude subagent file
    /// links its spawning call in the parent; a Codex spawn event names the
    /// child thread it started.
    fn record_links(&mut self, event: &StoredEvent) {
        if let (Some(parent), Some(pair_id)) = (self.child_of, &event.related_tool_use_id) {
            self.links.insert(ChildLink {
                session_uuid: parent,
                pair_id: pair_id.clone(),
                child_session_uuid: self.session_uuid,
                child_turn_id: -1,
            });
        }
        if event.agent == "codex"
            && matches!(
                event.subtype.as_deref(),
                Some("collab_agent_spawn_end" | "item_completed")
            )
        {
            let child = event
                .event_uuid
                .as_deref()
                .and_then(|id| Uuid::parse_str(id).ok())
                .filter(|child| *child != self.session_uuid);
            if let (Some(child), Some(pair_id)) = (child, &event.related_tool_use_id) {
                self.links.insert(ChildLink {
                    session_uuid: self.session_uuid,
                    pair_id: pair_id.clone(),
                    child_session_uuid: child,
                    child_turn_id: -1,
                });
            }
        }
    }

    /// An in-file sidechain turn whose seed names its spawning call.
    fn link_sidechain_turn(&mut self, turn_id: i64, seed: &StoredEvent) {
        if self.child_of.is_some() {
            return;
        }
        if let Some(pair_id) = &seed.related_tool_use_id {
            self.links.insert(ChildLink {
                session_uuid: self.session_uuid,
                pair_id: pair_id.clone(),
                child_session_uuid: self.session_uuid,
                child_turn_id: turn_id,
            });
        }
    }

    // ── usage ───────────────────────────────────────────────────────────

    /// Claude bills each response once, at the turn of its first receipt,
    /// with its latest receipt's usage. Codex reports cumulative totals, so a
    /// turn's share is its latest total less the total before it began.
    async fn apply_usage(&mut self, turn_id: i64, event: &StoredEvent) -> anyhow::Result<()> {
        let Some(usage) = event.usage_json.as_ref() else {
            return Ok(());
        };
        if event.agent == "codex" {
            let (total_in, total_out) =
                (token(usage, "input_tokens"), token(usage, "output_tokens"));
            let turn = self.turn_mut(turn_id);
            turn.input_tokens = (total_in - turn.usage_baseline_input).max(0);
            turn.output_tokens = (total_out - turn.usage_baseline_output).max(0);
            self.state.codex_input_total = Some(total_in);
            self.state.codex_output_total = Some(total_out);
            return Ok(());
        }
        let input = token(usage, "input_tokens")
            + token(usage, "cache_read_input_tokens")
            + token(usage, "cache_creation_input_tokens");
        let output = token(usage, "output_tokens");
        let Some(message_id) = event.usage_message_id.clone() else {
            let turn = self.turn_mut(turn_id);
            turn.input_tokens += input;
            turn.output_tokens += output;
            return Ok(());
        };
        let prior = match self.usage.get(&message_id) {
            Some(prior) => Some(prior.clone()),
            None => self.backend.message_usage(&message_id).await?,
        };
        let owner = match &prior {
            Some(prior) if self.load_turn(prior.turn_id).await? => prior.turn_id,
            _ => turn_id,
        };
        let turn = self.turn_mut(owner);
        if let Some(prior) = prior.as_ref().filter(|prior| prior.turn_id == owner) {
            turn.input_tokens -= prior.input_tokens;
            turn.output_tokens -= prior.output_tokens;
        }
        turn.input_tokens += input;
        turn.output_tokens += output;
        self.usage.insert(
            message_id.clone(),
            MessageUsage {
                turn_id: owner,
                input_tokens: input,
                output_tokens: output,
            },
        );
        self.dirty_usage.insert(message_id);
        Ok(())
    }
}

/// The event's visible content, if any: assistant text, thinking and tool
/// calls, or a summary, system or other record. Tool results show on their
/// operation instead.
fn visible_item(event: &StoredEvent) -> Option<TimelineChunk> {
    if is_tool_result_event(event) {
        return None;
    }
    if is_assistant_event(event) {
        let mut items = Vec::new();
        let mut thinking = Vec::new();
        for block in &event.blocks {
            match block.kind {
                BlockKind::Text => {
                    if let Some(text) = block.text.as_ref().filter(|text| !text.is_empty()) {
                        items.push(TimelineAssistantItem::Text { text: text.clone() });
                    }
                }
                BlockKind::Thinking => {
                    if let Some(text) = block
                        .text
                        .as_deref()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                    {
                        thinking.push(text.to_string());
                    }
                }
                BlockKind::ToolUse => {
                    if let Some(pair_id) = &block.tool_id {
                        items.push(TimelineAssistantItem::Tool {
                            pair_id: pair_id.clone(),
                        });
                    }
                }
                _ => {}
            }
        }
        return (!items.is_empty() || !thinking.is_empty())
            .then_some(TimelineChunk::Assistant { items, thinking });
    }
    let text = text_blocks_in(event).join(" ");
    Some(if is_summary_event(event) {
        TimelineChunk::Summary {
            subtype: event.subtype.clone(),
            text,
        }
    } else if is_system_event(event) {
        TimelineChunk::System {
            subtype: event.subtype.clone(),
            text,
            is_meta: event.is_meta,
        }
    } else {
        TimelineChunk::Generic {
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
        }
    })
}
