use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context};
use serde_json::json;
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::{sleep, Duration};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentType {
    Claude,
    Codex,
}

impl AgentType {
    fn parse(raw: &str) -> anyhow::Result<Self> {
        match raw {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            other => bail!("unknown agent type: {other}"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn binary_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    Real,
    Mock,
}

impl LaunchMode {
    fn parse(raw: &str) -> anyhow::Result<Self> {
        match raw {
            "real" => Ok(Self::Real),
            "mock" => Ok(Self::Mock),
            other => bail!("unknown launch mode: {other}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LauncherConfig {
    pub agent_type: AgentType,
    pub mode: LaunchMode,
    pub args: Vec<OsString>,
}

#[derive(Debug, Clone)]
struct LauncherEnv {
    pty_id: Option<Uuid>,
    correlate_sock: Option<PathBuf>,
    claude_projects_dir: Option<PathBuf>,
    codex_sessions_dir: Option<PathBuf>,
    cwd: PathBuf,
}

pub fn binary_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("sulion"))
}

pub fn parse_launcher_args(args: &[OsString]) -> anyhow::Result<LauncherConfig> {
    let mut agent_type: Option<AgentType> = None;
    let mut mode = LaunchMode::Real;
    let mut agent_args = Vec::new();

    let mut i = 0usize;
    while i < args.len() {
        let Some(arg) = args[i].to_str() else {
            bail!("launcher arg is not valid utf-8");
        };
        if arg == "--" {
            agent_args.extend(args[i + 1..].iter().cloned());
            break;
        }
        let next = |idx: usize| -> anyhow::Result<&str> {
            args.get(idx + 1)
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow::anyhow!("missing value for {}", args[idx].to_string_lossy()))
        };
        match arg {
            "--type" => {
                agent_type = Some(AgentType::parse(next(i)?)?);
                i += 2;
            }
            "--mode" => {
                mode = LaunchMode::parse(next(i)?)?;
                i += 2;
            }
            other => bail!("unknown launcher arg: {other}"),
        }
    }

    Ok(LauncherConfig {
        agent_type: agent_type.ok_or_else(|| anyhow::anyhow!("--type is required"))?,
        mode,
        args: agent_args,
    })
}

pub async fn run_launcher(cfg: LauncherConfig) -> anyhow::Result<i32> {
    let env = launcher_env()?;
    match cfg.mode {
        LaunchMode::Real => run_real(cfg, env).await,
        LaunchMode::Mock => run_mock(cfg, env).await,
    }
}

fn launcher_env() -> anyhow::Result<LauncherEnv> {
    let pty_id = std::env::var("SULION_PTY_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| Uuid::parse_str(&value))
        .transpose()
        .context("parse SULION_PTY_ID")?;

    Ok(LauncherEnv {
        pty_id,
        correlate_sock: std::env::var_os("SULION_CORRELATE_SOCK")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        claude_projects_dir: std::env::var_os("SULION_CLAUDE_PROJECTS")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        codex_sessions_dir: std::env::var_os("SULION_CODEX_SESSIONS")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        cwd: std::env::current_dir().context("resolve current directory")?,
    })
}

async fn run_real(cfg: LauncherConfig, env: LauncherEnv) -> anyhow::Result<i32> {
    match (cfg.agent_type, env.pty_id) {
        (AgentType::Codex, Some(pty_id)) => {
            let sessions_dir = env.codex_sessions_dir.ok_or_else(|| {
                anyhow::anyhow!("SULION_CODEX_SESSIONS is required inside sulion")
            })?;
            let correlate_sock = env.correlate_sock.ok_or_else(|| {
                anyhow::anyhow!("SULION_CORRELATE_SOCK is required inside sulion")
            })?;
            crate::codex::run_launcher(crate::codex::LauncherConfig {
                codex_bin: raw_agent_binary(AgentType::Codex),
                pty_id,
                sessions_dir,
                correlate_sock,
                args: cfg.args,
            })
            .await
        }
        _ => run_raw_agent(cfg.agent_type, &cfg.args).await,
    }
}

async fn run_raw_agent(agent_type: AgentType, args: &[OsString]) -> anyhow::Result<i32> {
    let mut cmd = Command::new(raw_agent_binary(agent_type));
    cmd.args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(false);
    let status = cmd
        .spawn()
        .with_context(|| format!("spawn {}", agent_type.binary_name()))?
        .wait()
        .await
        .with_context(|| format!("wait for {}", agent_type.binary_name()))?;
    Ok(exit_code(status))
}

fn raw_agent_binary(agent_type: AgentType) -> PathBuf {
    match agent_type {
        AgentType::Claude => std::env::var_os("SULION_REAL_CLAUDE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("claude")),
        AgentType::Codex => std::env::var_os("SULION_REAL_CODEX")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("codex")),
    }
}

async fn run_mock(cfg: LauncherConfig, env: LauncherEnv) -> anyhow::Result<i32> {
    let pty_id = env
        .pty_id
        .ok_or_else(|| anyhow::anyhow!("mock mode requires SULION_PTY_ID"))?;
    let correlate_sock = env
        .correlate_sock
        .clone()
        .ok_or_else(|| anyhow::anyhow!("mock mode requires SULION_CORRELATE_SOCK"))?;

    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(
            format!(
                "SULION {} mock ready. Type a prompt and press Enter.\r\n",
                cfg.agent_type.as_str()
            )
            .as_bytes(),
        )
        .await?;
    write_mock_prompt(&mut stdout, cfg.agent_type).await?;

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut prompt_index = 0u32;

    while let Some(line) = lines.next_line().await? {
        let prompt = line.trim();
        if prompt.is_empty() {
            write_mock_prompt(&mut stdout, cfg.agent_type).await?;
            continue;
        }
        if matches!(prompt, "exit" | "quit") {
            stdout.write_all(b"\r\nmock launcher exiting\r\n").await?;
            stdout.flush().await?;
            return Ok(0);
        }

        prompt_index += 1;
        let session_uuid = match cfg.agent_type {
            AgentType::Claude => {
                let claude_projects_dir = env.claude_projects_dir.clone().ok_or_else(|| {
                    anyhow::anyhow!("mock Claude mode requires SULION_CLAUDE_PROJECTS")
                })?;
                emit_mock_claude_roundtrip(
                    pty_id,
                    &correlate_sock,
                    &claude_projects_dir,
                    &env.cwd,
                    prompt,
                    prompt_index,
                )
                .await?
            }
            AgentType::Codex => {
                let codex_sessions_dir = env.codex_sessions_dir.clone().ok_or_else(|| {
                    anyhow::anyhow!("mock Codex mode requires SULION_CODEX_SESSIONS")
                })?;
                emit_mock_codex_roundtrip(
                    pty_id,
                    &correlate_sock,
                    &codex_sessions_dir,
                    &env.cwd,
                    prompt,
                    prompt_index,
                )
                .await?
            }
        };

        stdout
            .write_all(
                format!(
                    "\r\nwrote {} mock transcript {}\r\n",
                    cfg.agent_type.as_str(),
                    session_uuid
                )
                .as_bytes(),
            )
            .await?;
        write_mock_prompt(&mut stdout, cfg.agent_type).await?;
    }

    Ok(0)
}

async fn write_mock_prompt(
    stdout: &mut tokio::io::Stdout,
    agent_type: AgentType,
) -> anyhow::Result<()> {
    stdout
        .write_all(format!("mock-{}> ", agent_type.as_str()).as_bytes())
        .await?;
    stdout.flush().await?;
    Ok(())
}

async fn emit_mock_claude_roundtrip(
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

    let project_dir = claude_projects_dir.join(format!(
        "mock-{}-{}",
        sanitize_path_component(
            cwd.file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("workspace")
        ),
        prompt_index
    ));
    fs::create_dir_all(&project_dir).await?;
    let transcript_path = project_dir.join(format!("{session_uuid}.jsonl"));

    let base = chrono::Utc::now();
    let root_event_uuid = format!("claude-root-{session_uuid}");
    let sidechain_user_uuid = format!("claude-side-user-{session_uuid}");
    let edit_tool_id = format!("toolu_edit_{prompt_index}");
    let web_tool_id = format!("toolu_web_{prompt_index}");
    let task_tool_id = format!("toolu_task_{prompt_index}");

    let records = vec![
        json!({
            "type": "user",
            "timestamp": ts(base, 0),
            "message": {
                "role": "user",
                "content": format!("{prompt}\n"),
            }
        }),
        json!({
            "type": "assistant",
            "timestamp": ts(base, 1),
            "uuid": root_event_uuid,
            "message": {
                "role": "assistant",
                "content": [
                    {
                        "type": "text",
                        "text": format!("Claude mock assistant processed: {prompt}")
                    },
                    {
                        "type": "tool_use",
                        "id": edit_tool_id,
                        "name": "Edit",
                        "input": {
                            "file_path": "src/lib.rs",
                            "old_string": "pub fn greeting() -> &'static str {\\n    \\\"old\\\"\\n}\\n",
                            "new_string": "pub fn greeting() -> &'static str {\\n    \\\"mocked\\\"\\n}\\n"
                        }
                    },
                    {
                        "type": "tool_use",
                        "id": web_tool_id,
                        "name": "WebSearch",
                        "input": {
                            "query": format!("claude ingest parity {prompt_index}")
                        }
                    },
                    {
                        "type": "tool_use",
                        "id": task_tool_id,
                        "name": "Task",
                        "input": {
                            "description": "Inspect parity findings",
                            "subagent_type": "assistant"
                        }
                    }
                ]
            }
        }),
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
                "oldString": "pub fn greeting() -> &'static str {\n    \"old\"\n}\n",
                "newString": "pub fn greeting() -> &'static str {\n    \"mocked\"\n}\n",
                "replaceAll": false,
                "structuredPatch": [
                    {
                        "oldString": "pub fn greeting() -> &'static str {\n    \"old\"\n}\n",
                        "newString": "pub fn greeting() -> &'static str {\n    \"mocked\"\n}\n"
                    }
                ]
            }
        }),
        json!({
            "type": "user",
            "timestamp": ts(base, 3),
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": web_tool_id,
                        "content": "Found parity notes for Claude/Codex JSONL ingestion.",
                        "is_error": false
                    }
                ]
            }
        }),
        json!({
            "type": "system",
            "timestamp": ts(base, 4),
            "uuid": format!("claude-task-meta-{session_uuid}"),
            "parentUuid": format!("claude-root-{session_uuid}"),
            "tool_use_id": task_tool_id,
            "isSidechain": true,
            "isMeta": true,
            "subtype": "task-started"
        }),
        json!({
            "type": "user",
            "timestamp": ts(base, 5),
            "uuid": sidechain_user_uuid,
            "parentUuid": format!("claude-root-{session_uuid}"),
            "tool_use_id": task_tool_id,
            "isSidechain": true,
            "message": {
                "role": "user",
                "content": "Investigate parity between Claude and Codex ingest paths."
            }
        }),
        json!({
            "type": "assistant",
            "timestamp": ts(base, 6),
            "uuid": format!("claude-side-assistant-{session_uuid}"),
            "parentUuid": format!("claude-side-user-{session_uuid}"),
            "tool_use_id": task_tool_id,
            "isSidechain": true,
            "message": {
                "role": "assistant",
                "content": [
                    {
                        "type": "text",
                        "text": "Subagent report: Claude mock parity validated."
                    }
                ]
            }
        }),
    ];

    append_jsonl_records(&transcript_path, &records, Duration::from_millis(90)).await?;
    Ok(session_uuid)
}

async fn emit_mock_codex_roundtrip(
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
    let parent_path = codex_rollout_path(codex_sessions_dir, now, parent_uuid);
    let child_path = codex_rollout_path(
        codex_sessions_dir,
        now + chrono::Duration::seconds(1),
        child_uuid,
    );

    let turn_id = format!("codex-turn-{parent_uuid}");
    let child_turn_id = format!("codex-child-turn-{child_uuid}");
    let edit_call_id = format!("call_edit_{prompt_index}");
    let web_call_id = format!("call_web_{prompt_index}");
    let task_call_id = format!("call_task_{prompt_index}");

    let parent_records = vec![
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
        }),
        json!({
            "timestamp": ts(now, 1),
            "type": "turn_context",
            "payload": {
                "turn_id": turn_id,
                "cwd": cwd,
                "current_date": now.date_naive().to_string(),
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
        }),
        json!({
            "timestamp": ts(now, 2),
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": prompt }]
            }
        }),
        json!({
            "timestamp": ts(now, 3),
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": format!("Codex mock assistant processed: {prompt}") }]
            }
        }),
        json!({
            "timestamp": ts(now, 4),
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "apply_diff",
                "arguments": serde_json::to_string(&json!({
                    "file_path": "src/lib.rs",
                    "old_string": "pub fn greeting() -> &'static str {\\n    \\\"old\\\"\\n}\\n",
                    "new_string": "pub fn greeting() -> &'static str {\\n    \\\"mocked\\\"\\n}\\n"
                }))?,
                "call_id": edit_call_id
            }
        }),
        json!({
            "timestamp": ts(now, 5),
            "type": "response_item",
            "payload": {
                "type": "function_call_output",
                "call_id": edit_call_id,
                "output": "Applied diff to src/lib.rs",
                "is_error": false
            }
        }),
        json!({
            "timestamp": ts(now, 6),
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "web_search_query",
                "arguments": serde_json::to_string(&json!({
                    "query": format!("codex ingest parity {prompt_index}")
                }))?,
                "call_id": web_call_id
            }
        }),
        json!({
            "timestamp": ts(now, 7),
            "type": "response_item",
            "payload": {
                "type": "function_call_output",
                "call_id": web_call_id,
                "output": "Found parity notes for Codex/Claude JSONL ingestion.",
                "is_error": false
            }
        }),
        json!({
            "timestamp": ts(now, 8),
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "spawn_agent",
                "arguments": serde_json::to_string(&json!({
                    "fork_context": true,
                    "message": "Inspect parity findings in a subagent"
                }))?,
                "call_id": task_call_id
            }
        }),
        json!({
            "timestamp": ts(now, 9),
            "type": "event_msg",
            "payload": {
                "type": "collab_agent_spawn_end",
                "call_id": task_call_id,
                "sender_thread_id": parent_uuid,
                "new_thread_id": child_uuid,
                "new_agent_nickname": "Mock Scout",
                "prompt": "Inspect parity findings in a subagent",
                "model": "gpt-5.4-mini",
                "reasoning_effort": "medium",
                "status": "completed"
            }
        }),
    ];

    let child_now = now + chrono::Duration::seconds(1);
    let child_records = vec![
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
        }),
        json!({
            "timestamp": ts(child_now, 1),
            "type": "event_msg",
            "payload": {
                "type": "task_started",
                "turn_id": child_turn_id,
                "started_at": child_now.timestamp(),
                "model_context_window": 258400,
                "collaboration_mode_kind": "default"
            }
        }),
        json!({
            "timestamp": ts(child_now, 2),
            "type": "turn_context",
            "payload": {
                "turn_id": child_turn_id,
                "cwd": cwd,
                "current_date": child_now.date_naive().to_string(),
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
        }),
        json!({
            "timestamp": ts(child_now, 3),
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Inspect parity findings in a subagent" }]
            }
        }),
        json!({
            "timestamp": ts(child_now, 4),
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "Subagent report: Codex mock parity validated." }]
            }
        }),
    ];

    append_jsonl_records(&parent_path, &parent_records, Duration::from_millis(90)).await?;
    append_jsonl_records(&child_path, &child_records, Duration::from_millis(90)).await?;
    Ok(parent_uuid)
}

async fn append_jsonl_records(
    path: &Path,
    records: &[serde_json::Value],
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

fn codex_rollout_path(
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

fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(code) = status.code() {
            return code;
        }
        return status.signal().map(|sig| 128 + sig).unwrap_or(1);
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_launcher_args_requires_type_and_collects_agent_args() {
        let parsed = parse_launcher_args(&[
            OsString::from("--type"),
            OsString::from("claude"),
            OsString::from("--mode"),
            OsString::from("mock"),
            OsString::from("--"),
            OsString::from("--dangerously-skip-permissions"),
        ])
        .unwrap();

        assert_eq!(parsed.agent_type, AgentType::Claude);
        assert_eq!(parsed.mode, LaunchMode::Mock);
        assert_eq!(
            parsed.args,
            vec![OsString::from("--dangerously-skip-permissions")]
        );
    }

    #[test]
    fn codex_rollout_path_uses_codex_session_layout() {
        let root = PathBuf::from("/tmp/codex");
        let ts = chrono::DateTime::parse_from_rfc3339("2026-04-20T12:34:56Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let session_uuid = Uuid::parse_str("019da571-ab6d-72e2-94b2-4fc5544f53d2").unwrap();

        let path = codex_rollout_path(&root, ts, session_uuid);
        assert_eq!(
            path,
            PathBuf::from(
                "/tmp/codex/2026/04/20/rollout-2026-04-20T12-34-56-019da571-ab6d-72e2-94b2-4fc5544f53d2.jsonl"
            )
        );
    }
}
