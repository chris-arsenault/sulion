use chrono::{DateTime, TimeZone, Utc};
use serde_json::{json, Value};

use crate::ingest::canonical::{Block, OperationCategory};

use super::*;

fn ts(sec: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(sec, 0).single().unwrap()
}

fn text(ord: i32, value: &str) -> Block {
    Block::text(ord, value)
}

fn thinking(ord: i32, value: &str) -> Block {
    Block::thinking(ord, value)
}

fn tool_use(ord: i32, id: &str, name: &str, category: OperationCategory, input: Value) -> Block {
    let mut block = Block::tool_use(ord, id, name, input);
    block.operation_category = Some(category);
    block
}

fn tool_result(ord: i32, id: &str, text: &str, is_error: bool) -> Block {
    Block::tool_result(ord, id, Some(text.to_string()), is_error, None)
}

fn event(byte_offset: i64, kind: &str, blocks: Vec<Block>) -> StoredEvent {
    StoredEvent {
        byte_offset,
        timestamp: ts(byte_offset),
        kind: kind.to_string(),
        agent: "claude-code".to_string(),
        speaker: Some(
            match kind {
                "assistant" => "assistant",
                "user" => "user",
                "system" => "system",
                "summary" => "summary",
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
        blocks,
    }
}

#[test]
fn projects_turns_and_pairs() {
    let events = vec![
        event(1, "user", vec![text(0, "hello")]),
        event(
            2,
            "assistant",
            vec![
                text(0, "working"),
                thinking(1, "step"),
                tool_use(
                    2,
                    "t1",
                    "bash",
                    OperationCategory::Utility,
                    json!({"command": "ls -la"}),
                ),
            ],
        ),
        event(3, "user", vec![tool_result(0, "t1", "done", false)]),
    ];

    let projected = project_timeline(&events, events.len() as i64, &ProjectionFilters::default());
    assert_eq!(projected.turns.len(), 1);
    let turn = &projected.turns[0];
    assert_eq!(turn.preview, "hello");
    assert_eq!(turn.tool_pairs.len(), 1);
    assert_eq!(turn.tool_pairs[0].name, "bash");
    assert_eq!(
        turn.tool_pairs[0]
            .result
            .as_ref()
            .unwrap()
            .content
            .as_deref(),
        Some("done")
    );
    assert_eq!(turn.thinking_count, 1);
    assert!(turn
        .chunks
        .iter()
        .any(|chunk| matches!(chunk, TimelineChunk::Tool { pair_id } if pair_id == "t1")));
}

#[test]
fn claude_task_notifications_stay_inside_the_primary_turn() {
    let events = vec![
        event(1, "user", vec![text(0, "start the background work")]),
        event(2, "assistant", vec![text(0, "started")]),
        event(
            3,
            "user",
            vec![text(
                0,
                "<task-notification>\n<task-id>bg-1</task-id>\n<status>completed</status>\n</task-notification>",
            )],
        ),
        event(4, "assistant", vec![text(0, "the background work completed")]),
        event(5, "user", vec![text(0, "summarize the result")]),
        event(6, "assistant", vec![text(0, "summary")]),
    ];

    let projected = project_timeline(&events, events.len() as i64, &ProjectionFilters::default());

    assert_eq!(projected.turns.len(), 2);
    assert_eq!(projected.turns[0].preview, "start the background work");
    assert_eq!(projected.turns[0].event_count, 4);
    assert!(projected.turns[0]
        .chunks
        .iter()
        .any(|chunk| matches!(chunk, TimelineChunk::Generic { label, .. } if label == "user")));
    assert_eq!(projected.turns[1].preview, "summarize the result");
}

#[test]
fn codex_task_lifecycle_bookkeeping_does_not_start_turns() {
    let mut started = event(2, "system", Vec::new());
    started.agent = "codex".to_string();
    started.is_meta = true;
    started.subtype = Some("task_started".to_string());

    let mut complete = event(4, "system", Vec::new());
    complete.agent = "codex".to_string();
    complete.is_meta = true;
    complete.subtype = Some("task_complete".to_string());

    let mut prompt = event(1, "user", vec![text(0, "do the work")]);
    prompt.agent = "codex".to_string();
    let mut reply = event(3, "assistant", vec![text(0, "done")]);
    reply.agent = "codex".to_string();

    let events = vec![prompt, started, reply, complete];
    let projected = project_timeline(&events, events.len() as i64, &ProjectionFilters::default());

    assert_eq!(projected.turns.len(), 1);
    assert_eq!(projected.turns[0].preview, "do the work");
    assert_eq!(projected.turns[0].event_count, 2);
}

#[test]
fn hidden_categories_merge_assistant_chunks() {
    let events = vec![
        event(1, "user", vec![text(0, "prompt")]),
        event(
            2,
            "assistant",
            vec![
                text(0, "before"),
                tool_use(
                    1,
                    "t1",
                    "edit",
                    OperationCategory::CreateContent,
                    json!({"path": "/tmp/x"}),
                ),
                text(2, "after"),
            ],
        ),
    ];

    let mut filters = ProjectionFilters::default();
    filters
        .hidden_operation_categories
        .insert(OperationCategory::CreateContent);

    let projected = project_timeline(&events, events.len() as i64, &filters);
    let turn = &projected.turns[0];
    assert_eq!(turn.tool_pairs.len(), 1);
    assert_eq!(turn.chunks.len(), 1);
    match &turn.chunks[0] {
        TimelineChunk::Assistant { items, .. } => {
            assert_eq!(items.len(), 2);
        }
        other => panic!("unexpected chunk: {other:?}"),
    }
}

/// Codex code-mode exec pairs summarize by their extracted shell
/// command, exactly like bash pairs.
#[test]
fn exec_pairs_summarize_their_command_in_markdown() {
    let events = vec![
        event(1, "user", vec![text(0, "prompt")]),
        event(
            2,
            "assistant",
            vec![tool_use(
                0,
                "call-1",
                "exec",
                OperationCategory::Utility,
                json!({"command": "git status --short", "code": "const r = ..."}),
            )],
        ),
    ];

    let projected = project_timeline(&events, events.len() as i64, &ProjectionFilters::default());
    assert!(
        projected.turns[0].markdown.contains("`git status --short`"),
        "markdown lacks command summary: {}",
        projected.turns[0].markdown,
    );
}

/// Newer Claude Code builds write bookkeeping record kinds (`mode`,
/// `ai-title`, `file-history-delta`, …) that must not surface as generic
/// timeline rows when bookkeeping is hidden.
#[test]
fn newer_bookkeeping_kinds_stay_hidden_by_default() {
    let events = vec![
        event(1, "user", vec![text(0, "prompt")]),
        event(2, "mode", vec![]),
        event(3, "ai-title", vec![]),
        event(4, "file-history-delta", vec![]),
        event(5, "assistant", vec![text(0, "reply")]),
    ];

    let projected = project_timeline(&events, events.len() as i64, &ProjectionFilters::default());
    let turn = &projected.turns[0];
    assert!(
        turn.chunks
            .iter()
            .all(|chunk| !matches!(chunk, TimelineChunk::Generic { .. })),
        "bookkeeping kinds leaked into chunks: {:?}",
        turn.chunks,
    );
}

#[test]
fn task_pairs_capture_subagent_turns() {
    let mut root = event(
        1,
        "assistant",
        vec![tool_use(
            0,
            "task-1",
            "task",
            OperationCategory::Delegate,
            json!({"description": "investigate"}),
        )],
    );
    root.event_uuid = Some("asst-1".to_string());

    let mut sub_prompt = event(2, "user", vec![text(0, "sub prompt")]);
    sub_prompt.is_sidechain = true;
    sub_prompt.parent_event_uuid = Some("asst-1".to_string());

    let mut sub_reply = event(3, "assistant", vec![text(0, "sub reply")]);
    sub_reply.is_sidechain = true;
    sub_reply.parent_event_uuid = Some("evt-2".to_string());

    let events = vec![root, sub_prompt, sub_reply];
    let projected = project_timeline(&events, events.len() as i64, &ProjectionFilters::default());
    let pair = &projected.turns[0].tool_pairs[0];
    let subagent = pair.subagent.as_ref().expect("subagent projected");
    assert_eq!(subagent.event_count, 2);
    assert_eq!(subagent.turns.len(), 1);
    assert_eq!(subagent.turns[0].preview, "sub prompt");
    assert!(subagent.turns[0].is_sidechain);
}

#[test]
fn subagent_turns_link_their_own_nested_tasks_one_level_deep() {
    let mut root = event(
        1,
        "assistant",
        vec![tool_use(
            0,
            "task-1",
            "task",
            OperationCategory::Delegate,
            json!({"description": "outer"}),
        )],
    );
    root.event_uuid = Some("asst-1".to_string());

    let mut sub_prompt = event(2, "user", vec![text(0, "sub prompt")]);
    sub_prompt.is_sidechain = true;
    sub_prompt.parent_event_uuid = Some("asst-1".to_string());

    let mut sub_task = event(
        3,
        "assistant",
        vec![tool_use(
            0,
            "task-2",
            "task",
            OperationCategory::Delegate,
            json!({"description": "inner"}),
        )],
    );
    sub_task.is_sidechain = true;
    sub_task.parent_event_uuid = Some("evt-2".to_string());
    sub_task.event_uuid = Some("asst-2".to_string());

    let mut inner_prompt = event(4, "user", vec![text(0, "inner prompt")]);
    inner_prompt.is_sidechain = true;
    inner_prompt.parent_event_uuid = Some("asst-2".to_string());

    let events = vec![root, sub_prompt, sub_task, inner_prompt];
    let projected = project_timeline(&events, events.len() as i64, &ProjectionFilters::default());
    let outer = projected.turns[0].tool_pairs[0]
        .subagent
        .as_ref()
        .expect("outer subagent projected");
    let inner_pair = outer
        .turns
        .iter()
        .flat_map(|turn| turn.tool_pairs.iter())
        .find(|pair| pair.id == "task-2")
        .expect("nested task pair present");
    let inner = inner_pair
        .subagent
        .as_ref()
        .expect("nested subagent linked one level deep");
    assert!(
        inner
            .turns
            .iter()
            .any(|turn| turn.preview == "inner prompt"),
        "inner prompt turn projected: {:?}",
        inner
            .turns
            .iter()
            .map(|turn| &turn.preview)
            .collect::<Vec<_>>(),
    );
    // Depth stops there: the nested subagent's own pairs never link further.
    assert!(inner
        .turns
        .iter()
        .flat_map(|turn| turn.tool_pairs.iter())
        .all(|pair| pair.subagent.is_none()));
}

/// The preview truncates at a character count, not a byte index. Slicing bytes
/// panicked on any prompt whose cut landed inside a multi-byte character, which
/// killed the projection for that session and restarted the ingester on every
/// subsequent line.
#[test]
fn previews_truncate_multibyte_prompts_without_panicking() {
    fn preview_of(prompt: &str) -> String {
        let events = vec![
            event(1, "user", vec![text(0, prompt)]),
            event(2, "assistant", vec![text(0, "ok")]),
        ];
        let projected =
            project_timeline(&events, events.len() as i64, &ProjectionFilters::default());
        projected.turns[0].preview.clone()
    }

    // The original panic: 200 Cyrillic characters is 400 bytes, so a byte-index
    // cut at 279 landed inside 'я'. It is under the 280-character limit, so the
    // correct result is the prompt returned whole.
    let under_limit = "я".repeat(200);
    assert_eq!(preview_of(&under_limit), under_limit);

    // Over the limit it truncates on a character boundary.
    let over_limit = "я".repeat(400);
    let preview = preview_of(&over_limit);
    assert!(preview.ends_with('…'), "expected ellipsis, got {preview:?}");
    assert_eq!(preview.chars().count(), 280);

    // Emoji are 4 bytes, so they straddle a different set of boundaries.
    let emoji = preview_of(&"🙂".repeat(400));
    assert!(emoji.ends_with('…'));
    assert_eq!(emoji.chars().count(), 280);

    // The assistant fallback preview uses a different limit (260) and the same
    // truncation path.
    let events = vec![event(1, "assistant", vec![text(0, &"я".repeat(400))])];
    let projected = project_timeline(&events, events.len() as i64, &ProjectionFilters::default());
    let preview = &projected.turns[0].preview;
    assert!(preview.starts_with("(assistant) "));
    assert!(preview.ends_with('…'));
}
