use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::{json, Value};
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::time::{sleep, Duration};
use uuid::Uuid;

const MOCK_GREETING_OLD: &str = "pub fn greeting() -> &'static str {\n    \"old\"\n}\n";
const MOCK_GREETING_NEW: &str = "pub fn greeting() -> &'static str {\n    \"mocked\"\n}\n";
const MOCK_SUBAGENT_PROMPT: &str = "Inspect parity findings in a subagent";
const MOCK_CLAUDE_PARITY_NOTE: &str = "Found parity notes for Claude/Codex JSONL ingestion.";
const MOCK_CODEX_PARITY_NOTE: &str = "Found parity notes for Codex/Claude JSONL ingestion.";
const MOCK_CLAUDE_SIDECHAIN_PROMPT: &str =
    "Investigate parity between Claude and Codex ingest paths.";
const MOCK_CLAUDE_SIDECHAIN_REPORT: &str = "Subagent report: Claude mock parity validated.";
const MOCK_CODEX_SIDECHAIN_REPORT: &str = "Subagent report: Codex mock parity validated.";

struct ClaudeMockIds {
    root_event_uuid: String,
    sidechain_user_uuid: String,
    edit_tool_id: String,
    web_tool_id: String,
    task_tool_id: String,
}

impl ClaudeMockIds {
    fn new(session_uuid: Uuid, prompt_index: u32) -> Self {
        Self {
            root_event_uuid: format!("claude-root-{session_uuid}"),
            sidechain_user_uuid: format!("claude-side-user-{session_uuid}"),
            edit_tool_id: format!("toolu_edit_{prompt_index}"),
            web_tool_id: format!("toolu_web_{prompt_index}"),
            task_tool_id: format!("toolu_task_{prompt_index}"),
        }
    }
}

struct CodexMockIds {
    turn_id: String,
    child_turn_id: String,
    edit_call_id: String,
    web_call_id: String,
    task_call_id: String,
}

impl CodexMockIds {
    fn new(parent_uuid: Uuid, child_uuid: Uuid, prompt_index: u32) -> Self {
        Self {
            turn_id: format!("codex-turn-{parent_uuid}"),
            child_turn_id: format!("codex-child-turn-{child_uuid}"),
            edit_call_id: format!("call_edit_{prompt_index}"),
            web_call_id: format!("call_web_{prompt_index}"),
            task_call_id: format!("call_task_{prompt_index}"),
        }
    }
}

pub(super) async fn emit_mock_claude_roundtrip(
    pty_id: Uuid,
    correlate_sock: &Path,
    claude_projects_dir: &Path,
    cwd: &Path,
    prompt: &str,
    prompt_index: u32,
) -> anyhow::Result<Uuid> {
    let session_uuid = Uuid::new_v4();
    crate::correlate::send_for_agent(correlate_sock, pty_id, session_uuid, "claude-code")
        .await
        .context("correlate mock Claude session")?;

    let transcript_path =
        claude_transcript_path(claude_projects_dir, cwd, prompt_index, session_uuid).await?;
    let base = chrono::Utc::now();
    let ids = ClaudeMockIds::new(session_uuid, prompt_index);
    let records = build_mock_claude_records(session_uuid, base, prompt, prompt_index, &ids);
    append_jsonl_records(&transcript_path, &records, Duration::from_millis(90)).await?;
    Ok(session_uuid)
}

pub(super) async fn emit_mock_codex_roundtrip(
    pty_id: Uuid,
    correlate_sock: &Path,
    codex_sessions_dir: &Path,
    cwd: &Path,
    prompt: &str,
    prompt_index: u32,
) -> anyhow::Result<Uuid> {
    let parent_uuid = Uuid::new_v4();
    let child_uuid = Uuid::new_v4();
    crate::correlate::send_for_agent(correlate_sock, pty_id, parent_uuid, "codex")
        .await
        .context("correlate mock Codex session")?;

    let now = chrono::Utc::now();
    let child_now = now + chrono::Duration::seconds(1);
    let ids = CodexMockIds::new(parent_uuid, child_uuid, prompt_index);
    let parent_path = codex_rollout_path(codex_sessions_dir, now, parent_uuid);
    let child_path = codex_rollout_path(codex_sessions_dir, child_now, child_uuid);
    let parent_records = build_mock_codex_parent_records(
        now,
        cwd,
        prompt,
        prompt_index,
        parent_uuid,
        child_uuid,
        &ids,
    )?;
    let child_records =
        build_mock_codex_child_records(child_now, cwd, parent_uuid, child_uuid, &ids);
    append_jsonl_records(&parent_path, &parent_records, Duration::from_millis(90)).await?;
    append_jsonl_records(&child_path, &child_records, Duration::from_millis(90)).await?;
    Ok(parent_uuid)
}

async fn claude_transcript_path(
    claude_projects_dir: &Path,
    cwd: &Path,
    prompt_index: u32,
    session_uuid: Uuid,
) -> anyhow::Result<PathBuf> {
    let project_dir = claude_projects_dir.join(claude_project_dir_name(cwd, prompt_index));
    fs::create_dir_all(&project_dir).await?;
    Ok(project_dir.join(format!("{session_uuid}.jsonl")))
}

fn claude_project_dir_name(cwd: &Path, prompt_index: u32) -> String {
    format!(
        "mock-{}-{}",
        sanitize_path_component(
            cwd.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("workspace")
        ),
        prompt_index
    )
}

fn build_mock_claude_records(
    session_uuid: Uuid,
    base: chrono::DateTime<chrono::Utc>,
    prompt: &str,
    prompt_index: u32,
    ids: &ClaudeMockIds,
) -> Vec<Value> {
    vec![
        claude_user_prompt_record(base, prompt),
        claude_assistant_record(base, prompt, prompt_index, ids),
        claude_edit_result_record(base, &ids.edit_tool_id),
        claude_web_result_record(base, &ids.web_tool_id),
        claude_task_meta_record(base, session_uuid, ids),
        claude_sidechain_user_record(base, ids),
        claude_sidechain_assistant_record(base, session_uuid, ids),
    ]
}

fn claude_user_prompt_record(base: chrono::DateTime<chrono::Utc>, prompt: &str) -> Value {
    json!({
        "type": "user",
        "timestamp": ts(base, 0),
        "message": {
            "role": "user",
            "content": format!("{prompt}\n"),
        }
    })
}

fn claude_assistant_record(
    base: chrono::DateTime<chrono::Utc>,
    prompt: &str,
    prompt_index: u32,
    ids: &ClaudeMockIds,
) -> Value {
    json!({
        "type": "assistant",
        "timestamp": ts(base, 1),
        "uuid": &ids.root_event_uuid,
        "message": {
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": format!("Claude mock assistant processed: {prompt}")
                },
                {
                    "type": "tool_use",
                    "id": &ids.edit_tool_id,
                    "name": "Edit",
                    "input": {
                        "file_path": "src/lib.rs",
                        "old_string": MOCK_GREETING_OLD,
                        "new_string": MOCK_GREETING_NEW
                    }
                },
                {
                    "type": "tool_use",
                    "id": &ids.web_tool_id,
                    "name": "WebSearch",
                    "input": {
                        "query": format!("claude ingest parity {prompt_index}")
                    }
                },
                {
                    "type": "tool_use",
                    "id": &ids.task_tool_id,
                    "name": "Task",
                    "input": {
                        "description": "Inspect parity findings",
                        "subagent_type": "assistant"
                    }
                }
            ]
        }
    })
}

fn claude_edit_result_record(base: chrono::DateTime<chrono::Utc>, edit_tool_id: &str) -> Value {
    json!({
        "type": "user",
        "timestamp": ts(base, 2),
        "message": {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": edit_tool_id,
                    "content": "",
                    "is_error": false
                }
            ]
        },
        "toolUseResult": {
            "filePath": "src/lib.rs",
            "oldString": MOCK_GREETING_OLD,
            "newString": MOCK_GREETING_NEW,
            "replaceAll": false,
            "structuredPatch": [
                {
                    "oldString": MOCK_GREETING_OLD,
                    "newString": MOCK_GREETING_NEW
                }
            ]
        }
    })
}

fn claude_web_result_record(base: chrono::DateTime<chrono::Utc>, web_tool_id: &str) -> Value {
    json!({
        "type": "user",
        "timestamp": ts(base, 3),
        "message": {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": web_tool_id,
                    "content": MOCK_CLAUDE_PARITY_NOTE,
                    "is_error": false
                }
            ]
        }
    })
}

fn claude_task_meta_record(
    base: chrono::DateTime<chrono::Utc>,
    session_uuid: Uuid,
    ids: &ClaudeMockIds,
) -> Value {
    json!({
        "type": "system",
        "timestamp": ts(base, 4),
        "uuid": format!("claude-task-meta-{session_uuid}"),
        "parentUuid": &ids.root_event_uuid,
        "tool_use_id": &ids.task_tool_id,
        "isSidechain": true,
        "isMeta": true,
        "subtype": "task-started"
    })
}

fn claude_sidechain_user_record(base: chrono::DateTime<chrono::Utc>, ids: &ClaudeMockIds) -> Value {
    json!({
        "type": "user",
        "timestamp": ts(base, 5),
        "uuid": &ids.sidechain_user_uuid,
        "parentUuid": &ids.root_event_uuid,
        "tool_use_id": &ids.task_tool_id,
        "isSidechain": true,
        "message": {
            "role": "user",
            "content": MOCK_CLAUDE_SIDECHAIN_PROMPT
        }
    })
}

fn claude_sidechain_assistant_record(
    base: chrono::DateTime<chrono::Utc>,
    session_uuid: Uuid,
    ids: &ClaudeMockIds,
) -> Value {
    json!({
        "type": "assistant",
        "timestamp": ts(base, 6),
        "uuid": format!("claude-side-assistant-{session_uuid}"),
        "parentUuid": &ids.sidechain_user_uuid,
        "tool_use_id": &ids.task_tool_id,
        "isSidechain": true,
        "message": {
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": MOCK_CLAUDE_SIDECHAIN_REPORT
                }
            ]
        }
    })
}

fn build_mock_codex_parent_records(
    now: chrono::DateTime<chrono::Utc>,
    cwd: &Path,
    prompt: &str,
    prompt_index: u32,
    parent_uuid: Uuid,
    child_uuid: Uuid,
    ids: &CodexMockIds,
) -> anyhow::Result<Vec<Value>> {
    Ok(vec![
        codex_parent_session_meta_record(now, cwd, parent_uuid),
        codex_turn_context_record(now, 1, cwd, &ids.turn_id),
        codex_message_record(now, 2, "user", "input_text", prompt),
        codex_message_record(
            now,
            3,
            "assistant",
            "output_text",
            &format!("Codex mock assistant processed: {prompt}"),
        ),
        codex_function_call_record(
            now,
            4,
            "apply_diff",
            codex_edit_arguments(),
            &ids.edit_call_id,
        )?,
        codex_function_output_record(now, 5, &ids.edit_call_id, "Applied diff to src/lib.rs"),
        codex_function_call_record(
            now,
            6,
            "web_search_query",
            json!({ "query": format!("codex ingest parity {prompt_index}") }),
            &ids.web_call_id,
        )?,
        codex_function_output_record(now, 7, &ids.web_call_id, MOCK_CODEX_PARITY_NOTE),
        codex_function_call_record(
            now,
            8,
            "spawn_agent",
            json!({ "fork_context": true, "message": MOCK_SUBAGENT_PROMPT }),
            &ids.task_call_id,
        )?,
        codex_spawn_end_record(now, 9, parent_uuid, child_uuid, &ids.task_call_id),
    ])
}

fn build_mock_codex_child_records(
    child_now: chrono::DateTime<chrono::Utc>,
    cwd: &Path,
    parent_uuid: Uuid,
    child_uuid: Uuid,
    ids: &CodexMockIds,
) -> Vec<Value> {
    vec![
        codex_child_session_meta_record(child_now, cwd, parent_uuid, child_uuid),
        codex_task_started_record(child_now, 1, &ids.child_turn_id),
        codex_turn_context_record(child_now, 2, cwd, &ids.child_turn_id),
        codex_message_record(child_now, 3, "user", "input_text", MOCK_SUBAGENT_PROMPT),
        codex_message_record(
            child_now,
            4,
            "assistant",
            "output_text",
            MOCK_CODEX_SIDECHAIN_REPORT,
        ),
    ]
}

fn codex_edit_arguments() -> Value {
    json!({
        "file_path": "src/lib.rs",
        "old_string": MOCK_GREETING_OLD,
        "new_string": MOCK_GREETING_NEW
    })
}

fn codex_parent_session_meta_record(
    now: chrono::DateTime<chrono::Utc>,
    cwd: &Path,
    parent_uuid: Uuid,
) -> Value {
    json!({
        "timestamp": ts(now, 0),
        "type": "session_meta",
        "payload": {
            "id": parent_uuid,
            "timestamp": ts(now, 0),
            "cwd": cwd,
            "originator": "codex-tui",
            "cli_version": "0.0.0-mock",
            "source": "cli",
            "model_provider": "openai"
        }
    })
}

fn codex_child_session_meta_record(
    child_now: chrono::DateTime<chrono::Utc>,
    cwd: &Path,
    parent_uuid: Uuid,
    child_uuid: Uuid,
) -> Value {
    json!({
        "timestamp": ts(child_now, 0),
        "type": "session_meta",
        "payload": {
            "id": child_uuid,
            "forked_from_id": parent_uuid,
            "timestamp": ts(child_now, 0),
            "cwd": cwd,
            "originator": "codex-tui",
            "cli_version": "0.0.0-mock",
            "source": {
                "subagent": {
                    "thread_spawn": {
                        "parent_thread_id": parent_uuid,
                        "depth": 1,
                        "agent_path": null,
                        "agent_nickname": "Mock Scout",
                        "agent_role": null
                    }
                }
            },
            "agent_nickname": "Mock Scout",
            "model_provider": "openai"
        }
    })
}

fn codex_task_started_record(
    child_now: chrono::DateTime<chrono::Utc>,
    offset_seconds: i64,
    turn_id: &str,
) -> Value {
    json!({
        "timestamp": ts(child_now, offset_seconds),
        "type": "event_msg",
        "payload": {
            "type": "task_started",
            "turn_id": turn_id,
            "started_at": child_now.timestamp(),
            "model_context_window": 258400,
            "collaboration_mode_kind": "default"
        }
    })
}

fn codex_turn_context_record(
    at: chrono::DateTime<chrono::Utc>,
    offset_seconds: i64,
    cwd: &Path,
    turn_id: &str,
) -> Value {
    json!({
        "timestamp": ts(at, offset_seconds),
        "type": "turn_context",
        "payload": {
            "turn_id": turn_id,
            "cwd": cwd,
            "current_date": at.date_naive().to_string(),
            "timezone": "UTC",
            "approval_policy": "never",
            "sandbox_policy": { "type": "danger-full-access" },
            "model": "gpt-5.4-mini",
            "personality": "pragmatic",
            "collaboration_mode": { "mode": "default" },
            "realtime_active": false,
            "effort": "medium",
            "summary": "none",
            "truncation_policy": { "mode": "tokens", "limit": 10000 }
        }
    })
}

fn codex_message_record(
    at: chrono::DateTime<chrono::Utc>,
    offset_seconds: i64,
    role: &str,
    content_type: &str,
    text: &str,
) -> Value {
    json!({
        "timestamp": ts(at, offset_seconds),
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": role,
            "content": [{ "type": content_type, "text": text }]
        }
    })
}

fn codex_function_call_record(
    at: chrono::DateTime<chrono::Utc>,
    offset_seconds: i64,
    name: &str,
    arguments: Value,
    call_id: &str,
) -> anyhow::Result<Value> {
    Ok(json!({
        "timestamp": ts(at, offset_seconds),
        "type": "response_item",
        "payload": {
            "type": "function_call",
            "name": name,
            "arguments": serde_json::to_string(&arguments)?,
            "call_id": call_id
        }
    }))
}

fn codex_function_output_record(
    at: chrono::DateTime<chrono::Utc>,
    offset_seconds: i64,
    call_id: &str,
    output: &str,
) -> Value {
    json!({
        "timestamp": ts(at, offset_seconds),
        "type": "response_item",
        "payload": {
            "type": "function_call_output",
            "call_id": call_id,
            "output": output,
            "is_error": false
        }
    })
}

fn codex_spawn_end_record(
    now: chrono::DateTime<chrono::Utc>,
    offset_seconds: i64,
    parent_uuid: Uuid,
    child_uuid: Uuid,
    task_call_id: &str,
) -> Value {
    json!({
        "timestamp": ts(now, offset_seconds),
        "type": "event_msg",
        "payload": {
            "type": "collab_agent_spawn_end",
            "call_id": task_call_id,
            "sender_thread_id": parent_uuid,
            "new_thread_id": child_uuid,
            "new_agent_nickname": "Mock Scout",
            "prompt": MOCK_SUBAGENT_PROMPT,
            "model": "gpt-5.4-mini",
            "reasoning_effort": "medium",
            "status": "completed"
        }
    })
}

async fn append_jsonl_records(
    path: &Path,
    records: &[Value],
    delay: Duration,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    for record in records {
        file.write_all(record.to_string().as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        sleep(delay).await;
    }
    Ok(())
}

pub(super) fn codex_rollout_path(
    root: &Path,
    ts: chrono::DateTime<chrono::Utc>,
    session_uuid: Uuid,
) -> PathBuf {
    let day_dir = root
        .join(ts.format("%Y").to_string())
        .join(ts.format("%m").to_string())
        .join(ts.format("%d").to_string());
    day_dir.join(format!(
        "rollout-{}-{}.jsonl",
        ts.format("%Y-%m-%dT%H-%M-%S"),
        session_uuid
    ))
}

fn sanitize_path_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn ts(base: chrono::DateTime<chrono::Utc>, offset_seconds: i64) -> String {
    (base + chrono::Duration::seconds(offset_seconds)).to_rfc3339()
}
