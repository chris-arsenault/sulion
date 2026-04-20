use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{bail, Context};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

mod mock_transcripts;

use mock_transcripts::{emit_mock_claude_roundtrip, emit_mock_codex_roundtrip};

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

fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(code) = status.code() {
            return code;
        }
        status.signal().map(|sig| 128 + sig).unwrap_or(1)
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

        let path = mock_transcripts::codex_rollout_path(&root, ts, session_uuid);
        assert_eq!(
            path,
            PathBuf::from(
                "/tmp/codex/2026/04/20/rollout-2026-04-20T12-34-56-019da571-ab6d-72e2-94b2-4fc5544f53d2.jsonl"
            )
        );
    }
}
