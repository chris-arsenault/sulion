//! The reducer against an in-memory store. Every scenario is applied one
//! event per batch and as a single batch, and both must agree.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::ingest::canonical::{Block, OperationCategory};

use super::reduce::{
    Backend, Changes, ChildLink, MessageUsage, OpRow, Reducer, SessionState, TurnRow,
};
use super::*;

#[derive(Default)]
struct MemDb {
    state: SessionState,
    turns: BTreeMap<i64, TurnRow>,
    items: BTreeMap<(i64, i64), TimelineChunk>,
    ops: BTreeMap<(i64, i32), OpRow>,
    usage: HashMap<String, MessageUsage>,
    links: BTreeSet<ChildLink>,
}

impl MemDb {
    fn apply(&mut self, changes: Changes) {
        self.state = changes.state;
        for turn in changes.turns {
            self.turns.insert(turn.turn_id, turn);
        }
        for (turn, offset, chunk) in changes.items {
            self.items.insert((turn, offset), chunk);
        }
        for op in changes.new_ops {
            self.ops.insert(op.key(), op);
        }
        for (op, _) in changes.updated_ops {
            self.ops.insert(op.key(), op);
        }
        self.usage.extend(changes.usage);
        self.links.extend(changes.links);
    }

    fn timeline(&self) -> Vec<TimelineTurn> {
        let mut turns: Vec<&TurnRow> = self.turns.values().collect();
        turns.sort_by_key(|turn| turn.turn_ord);
        turns
            .into_iter()
            .map(|turn| {
                let items: Vec<TimelineItem> = self
                    .items
                    .range((turn.turn_id, i64::MIN)..=(turn.turn_id, i64::MAX))
                    .map(|((_, offset), chunk)| TimelineItem {
                        offset: *offset,
                        chunk: chunk.clone(),
                    })
                    .collect();
                let pairs: Vec<TimelineToolPair> = self
                    .ops
                    .range((turn.turn_id, i32::MIN)..=(turn.turn_id, i32::MAX))
                    .map(|(_, op)| op.pair())
                    .collect();
                TimelineTurn {
                    id: turn.turn_id,
                    turn_key: None,
                    preview: turn.preview.clone(),
                    user_prompt_text: turn.user_prompt_text.clone(),
                    start_timestamp: turn.start_timestamp,
                    end_timestamp: turn.end_timestamp,
                    duration_ms: turn.duration_ms,
                    event_count: turn.event_count as usize,
                    operation_count: turn.operation_count as usize,
                    thinking_count: turn.thinking_count as usize,
                    has_errors: turn.has_errors,
                    is_sidechain: turn.is_sidechain,
                    input_tokens: turn.input_tokens,
                    output_tokens: turn.output_tokens,
                    markdown: compose_turn_markdown(
                        turn.user_prompt_text.as_deref(),
                        &items,
                        &pairs,
                    ),
                    items,
                    tool_pairs: pairs,
                    pty_session_id: None,
                    session_uuid: None,
                    session_agent: None,
                    session_label: None,
                    session_state: None,
                }
            })
            .collect()
    }
}

impl Backend for &mut MemDb {
    async fn turn(&mut self, turn_id: i64) -> anyhow::Result<Option<TurnRow>> {
        Ok(self.turns.get(&turn_id).cloned())
    }

    async fn ops_by_pair(&mut self, pair_id: &str, before: i64) -> anyhow::Result<Vec<OpRow>> {
        Ok(self
            .ops
            .values()
            .filter(|op| op.pair_id == pair_id && op.call_offset < before)
            .cloned()
            .collect())
    }

    async fn prompt_seen(&mut self, event_uuid: &str) -> anyhow::Result<bool> {
        Ok(self
            .turns
            .values()
            .any(|turn| turn.prompt_event_uuid.as_deref() == Some(event_uuid)))
    }

    async fn message_usage(&mut self, message_id: &str) -> anyhow::Result<Option<MessageUsage>> {
        Ok(self.usage.get(message_id).cloned())
    }

    async fn exec_candidates(
        &mut self,
        before: i64,
        _started: Option<DateTime<Utc>>,
    ) -> anyhow::Result<Vec<OpRow>> {
        Ok(self
            .ops
            .values()
            .filter(|op| op.call_offset < before)
            .cloned()
            .collect())
    }

    async fn ops_by_running_cell(&mut self, cell: &str) -> anyhow::Result<Vec<OpRow>> {
        Ok(self
            .ops
            .values()
            .filter(|op| op.running_cell.as_deref() == Some(cell))
            .cloned()
            .collect())
    }
}

fn session() -> Uuid {
    Uuid::from_u128(0x5e55_1011)
}

fn reduce_batches(events: &[StoredEvent], batch: usize) -> MemDb {
    let mut db = MemDb::default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    for chunk in events.chunks(batch.max(1)) {
        let state = db.state.clone();
        let changes = runtime.block_on(async {
            let mut reducer = Reducer::new(&mut db, session(), state, None, None);
            for event in chunk {
                reducer.apply(event.clone()).await.unwrap();
            }
            reducer.into_changes().1
        });
        db.apply(changes);
    }
    db
}

/// The session's turns, checked to be the same one event at a time.
fn project(events: &[StoredEvent]) -> Vec<TimelineTurn> {
    let whole = reduce_batches(events, events.len());
    let stepped = reduce_batches(events, 1);
    let turns = whole.timeline();
    assert_eq!(
        serde_json::to_value(&turns).unwrap(),
        serde_json::to_value(stepped.timeline()).unwrap(),
        "per-event batches diverged from one batch",
    );
    turns
}

fn ts(sec: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(sec, 0).single().unwrap()
}

fn text(value: &str) -> Block {
    Block::text(0, value)
}

fn call(id: &str, name: &str, input: Value) -> Block {
    let mut block = Block::tool_use(0, id, name, input);
    block.operation_category = Some(OperationCategory::Utility);
    block
}

fn result(id: &str, text: &str, is_error: bool) -> Block {
    Block::tool_result(0, id, Some(text.to_string()), is_error, None)
}

fn event(byte_offset: i64, kind: &str, blocks: Vec<Block>) -> StoredEvent {
    StoredEvent {
        byte_offset,
        timestamp: ts(byte_offset),
        kind: kind.to_string(),
        agent: "claude-code".to_string(),
        speaker: Some(
            match kind {
                "assistant" | "user" | "system" | "summary" => kind,
                _ => "other",
            }
            .to_string(),
        ),
        content_kind: None,
        event_uuid: Some(format!("evt-{byte_offset}")),
        parent_event_uuid: None,
        related_tool_use_id: None,
        is_sidechain: false,
        is_meta: false,
        subtype: None,
        usage_json: None,
        usage_message_id: None,
        source_session: None,
        blocks,
    }
}

fn codex(mut event: StoredEvent) -> StoredEvent {
    event.agent = "codex".to_string();
    event
}

fn runtime(offset: i64, id: &str, item: Value, failed: bool) -> StoredEvent {
    let mut evidence = event(
        offset,
        "system",
        vec![Block::tool_result(0, id, None, failed, Some(item))],
    );
    evidence.is_meta = true;
    codex(evidence)
}

#[test]
fn prompts_open_turns_that_collect_calls_results_and_items() {
    let mut thought = event(
        2,
        "assistant",
        vec![
            text("working"),
            call("t1", "bash", json!({"command": "ls"})),
        ],
    );
    thought.blocks.insert(1, Block::thinking(1, "step"));
    let turns = project(&[
        event(1, "user", vec![text("hello")]),
        thought,
        event(3, "user", vec![result("t1", "done", false)]),
        event(4, "user", vec![text("next")]),
    ]);
    assert_eq!(turns.len(), 2);
    let turn = &turns[0];
    assert_eq!(turn.preview, "hello");
    assert_eq!((turn.event_count, turn.thinking_count), (3, 1));
    assert!(!turn.tool_pairs[0].is_pending);
    assert_eq!(turn.items.len(), 1, "a tool result shows on its call");
    assert!(turn
        .markdown
        .contains("**Prompt**\n\n> hello\n\nworking\n\n**Tool:** `bash` `ls`"));
}

#[test]
fn notifications_commands_and_lifecycle_records_do_not_open_turns() {
    let mut started = codex(event(3, "system", vec![]));
    started.is_meta = true;
    started.subtype = Some("task_started".to_string());
    let turns = project(&[
        event(1, "user", vec![text("start")]),
        event(
            2,
            "user",
            vec![text(
                "<task-notification>\n<task-id>bg</task-id>\n</task-notification>",
            )],
        ),
        started,
        event(4, "user", vec![text("<command-name>/model</command-name>")]),
        event(5, "user", vec![text("second")]),
    ]);
    let previews: Vec<&str> = turns.iter().map(|turn| turn.preview.as_str()).collect();
    assert_eq!(previews, vec!["start", "second"]);
    assert_eq!(turns[0].event_count, 4);
}

#[test]
fn bookkeeping_before_the_first_turn_is_not_projected() {
    let turns = project(&[
        event(1, "attachment", vec![]),
        event(2, "user", vec![text("prompt")]),
        event(3, "assistant", vec![text("reply")]),
    ]);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].event_count, 2);
}

#[test]
fn a_repeated_prompt_is_skipped() {
    let mut repeat = event(3, "user", vec![text("first")]);
    repeat.event_uuid = Some("evt-1".into());
    let turns = project(&[
        event(1, "user", vec![text("first")]),
        event(2, "assistant", vec![text("reply")]),
        repeat,
    ]);
    assert_eq!(turns.len(), 1);
}

#[test]
fn a_turn_without_a_prompt_previews_its_first_assistant_text() {
    let turns = project(&[event(1, "assistant", vec![text(&"я".repeat(400))])]);
    assert!(turns[0].preview.starts_with("(assistant) я"));
}

#[test]
fn a_late_result_completes_its_call_in_the_earlier_turn() {
    let turns = project(&[
        event(1, "user", vec![text("first")]),
        event(2, "assistant", vec![call("c1", "bash", json!({}))]),
        event(3, "user", vec![text("second")]),
        event(4, "user", vec![result("c1", "late", true)]),
    ]);
    let op = &turns[0].tool_pairs[0];
    assert!(!op.is_pending && op.is_error);
    assert!(turns[0].has_errors);
    assert_eq!(turns[1].event_count, 2);
}

#[test]
fn claude_usage_charges_each_response_once_with_its_latest_receipt() {
    let receipt = |offset, output| {
        let mut receipt = event(offset, "assistant", vec![text("part")]);
        receipt.usage_json = Some(json!({"input_tokens": 100, "cache_read_input_tokens": 900,
            "cache_creation_input_tokens": 50, "output_tokens": output}));
        receipt.usage_message_id = Some("msg".to_string());
        receipt
    };
    let turns = project(&[
        event(1, "user", vec![text("one")]),
        receipt(2, 40),
        event(3, "user", vec![text("two")]),
        receipt(4, 268),
    ]);
    assert_eq!((turns[0].input_tokens, turns[0].output_tokens), (1050, 268));
    assert_eq!((turns[1].input_tokens, turns[1].output_tokens), (0, 0));
}

#[test]
fn codex_usage_is_each_turns_share_of_the_cumulative_totals() {
    let total = |offset, input, output| {
        let mut count = codex(event(offset, "token_count", vec![]));
        count.is_meta = true;
        count.usage_json = Some(json!({"input_tokens": input, "output_tokens": output}));
        count
    };
    let turns = project(&[
        codex(event(1, "user", vec![text("first")])),
        total(2, 1_000, 100),
        codex(event(3, "user", vec![text("second")])),
        total(4, 1_600, 180),
    ]);
    assert_eq!(
        (turns[0].input_tokens, turns[0].output_tokens),
        (1_000, 100)
    );
    assert_eq!((turns[1].input_tokens, turns[1].output_tokens), (600, 80));
}

#[test]
fn runtime_evidence_folds_into_its_call_and_reports_failures() {
    let turns = project(&[
        event(1, "user", vec![text("fix")]),
        codex(event(
            2,
            "assistant",
            vec![call(
                "call",
                "exec",
                json!("await tools.exec_command(args)"),
            )],
        )),
        runtime(
            3,
            "exec-runtime",
            json!({"runtime_item":{"type":"CommandExecution","id":"exec-runtime",
            "command":["bash","-lc","false"],"cwd":"/repo","stderr":"failure"},"started_at_ms":2500}),
            true,
        ),
        runtime(
            4,
            "edit-runtime",
            json!({"runtime_item":{"type":"FileChange","id":"edit-runtime","changes":{
            "/repo/src/main.rs":{"type":"update","unified_diff":"-old\n+new"}}},"started_at_ms":2600}),
            false,
        ),
        event(5, "system", vec![result("call", "finished", false)]),
    ]);
    let turn = &turns[0];
    let pair = &turn.tool_pairs[0];
    assert!(turn.has_errors && pair.is_error);
    let input = pair.input.as_ref().unwrap();
    assert_eq!(input["runtime_items"].as_array().unwrap().len(), 2);
    assert_eq!(input["file_edits"][0]["path"], "/repo/src/main.rs");
    assert_eq!(
        pair.result.as_ref().unwrap().payload.as_ref().unwrap()["runtime_items"][0]["runtime_item"]
            ["stderr"],
        "failure"
    );
}

#[test]
fn ambiguous_runtime_evidence_stays_as_bookkeeping() {
    let turns = project(&[
        event(1, "user", vec![text("look")]),
        codex(event(
            2,
            "assistant",
            vec![call("a", "exec", json!("view"))],
        )),
        codex(event(
            3,
            "assistant",
            vec![call("b", "exec", json!("view"))],
        )),
        runtime(
            4,
            "img",
            json!({"runtime_item":{"type":"ImageView","id":"img"},"started_at_ms":3500}),
            false,
        ),
    ]);
    assert!(turns[0].tool_pairs.iter().all(|pair| pair.is_pending));
    assert!(turns[0].items.iter().any(|item| matches!(&item.chunk,
        TimelineChunk::System { subtype, is_meta: true, .. } if subtype.as_deref() == Some("runtime_evidence"))));
    assert!(!turns[0].markdown.contains("Uncorrelated"));
}

#[test]
fn a_quoted_running_header_does_not_keep_an_exec_open() {
    let turns = project(&[
        event(1, "user", vec![text("edit")]),
        codex(event(
            2,
            "assistant",
            vec![call("read", "exec", json!("read"))],
        )),
        event(
            3,
            "system",
            vec![result(
                "read",
                "Script completed\n\"Script running with cell ID \"",
                false,
            )],
        ),
        codex(event(
            4,
            "assistant",
            vec![call("edit", "exec", json!("patch"))],
        )),
        runtime(
            5,
            "fc",
            json!({"runtime_item":{"type":"FileChange","id":"fc"},"started_at_ms":4500}),
            false,
        ),
        event(6, "system", vec![result("edit", "Script completed", false)]),
    ]);
    let edit = &turns[0].tool_pairs[1];
    assert_eq!(
        edit.input.as_ref().unwrap()["runtime_items"][0]["runtime_item"]["id"],
        "fc"
    );
}

#[test]
fn a_completed_wait_closes_the_cell_it_waited_on() {
    let evidence = |offset, id: &str, start| {
        runtime(
            offset,
            id,
            json!({"runtime_item":{"type":"CommandExecution","id":id},"started_at_ms":start}),
            false,
        )
    };
    let turns = project(&[
        event(1, "user", vec![text("run")]),
        codex(event(2, "assistant", vec![call("a", "exec", json!("run"))])),
        event(
            3,
            "system",
            vec![result("a", "Script running with cell ID cell-a", false)],
        ),
        codex(event(
            4,
            "assistant",
            vec![call("w", "wait", json!({"cell_id": "cell-a"}))],
        )),
        evidence(5, "runtime-a", 4500),
        event(6, "system", vec![result("w", "finished", false)]),
        codex(event(7, "assistant", vec![call("b", "exec", json!("run"))])),
        evidence(8, "runtime-b", 7500),
        event(9, "system", vec![result("b", "finished", false)]),
    ]);
    let pairs = &turns[0].tool_pairs;
    assert_eq!(
        pairs[0].input.as_ref().unwrap()["runtime_items"][0]["runtime_item"]["id"],
        "runtime-a"
    );
    assert_eq!(
        pairs[2].input.as_ref().unwrap()["runtime_items"][0]["runtime_item"]["id"],
        "runtime-b"
    );
}

#[test]
fn spawned_transcripts_are_linked_not_copied() {
    let mut sub_prompt = event(2, "user", vec![text("sub prompt")]);
    sub_prompt.is_sidechain = true;
    sub_prompt.related_tool_use_id = Some("task-1".into());
    let child = Uuid::from_u128(0xc41d);
    let mut spawn = codex(event(4, "system", vec![]));
    spawn.is_meta = true;
    spawn.subtype = Some("collab_agent_spawn_end".into());
    spawn.event_uuid = Some(child.to_string());
    spawn.related_tool_use_id = Some("call-spawn".into());
    let events = [
        event(
            1,
            "assistant",
            vec![call("task-1", "Agent", json!({"description": "look"}))],
        ),
        sub_prompt,
        event(3, "assistant", vec![text("sub reply")]),
        spawn,
    ];
    let db = reduce_batches(&events, 1);
    let links: Vec<(&str, Uuid, i64)> = db
        .links
        .iter()
        .map(|link| {
            (
                link.pair_id.as_str(),
                link.child_session_uuid,
                link.child_turn_id,
            )
        })
        .collect();
    assert_eq!(
        links,
        vec![("call-spawn", child, -1), ("task-1", session(), 2)]
    );
    assert!(db.turns[&2].is_sidechain);
}

#[test]
fn an_append_writes_only_its_own_rows() {
    let mut events = vec![event(1, "user", vec![text("long")])];
    for n in 0..200 {
        events.push(event(
            2 + n * 2,
            "assistant",
            vec![call(&format!("c{n}"), "bash", json!({}))],
        ));
        events.push(event(
            3 + n * 2,
            "user",
            vec![result(&format!("c{n}"), "ok", false)],
        ));
    }
    let mut db = reduce_batches(&events, 50);
    let state = db.state.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let changes = runtime.block_on(async {
        let mut reducer = Reducer::new(&mut db, session(), state, None, None);
        reducer
            .apply(event(
                1000,
                "assistant",
                vec![call("tail", "bash", json!({}))],
            ))
            .await
            .unwrap();
        reducer
            .apply(event(1001, "user", vec![result("tail", "ok", false)]))
            .await
            .unwrap();
        reducer.into_changes().1
    });
    assert_eq!(changes.turns.len(), 1);
    assert_eq!(changes.items.len(), 1);
    assert_eq!(changes.new_ops.len(), 1);
    assert!(changes.updated_ops.is_empty());
}
