use std::collections::HashSet;
use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use uuid::Uuid;

const FD_SCAN_INTERVAL: Duration = Duration::from_millis(100);
const DEFAULT_WRAPPER_PATH: &str = "/opt/sulion/bin/codex";

#[derive(Debug, Clone)]
pub struct LauncherConfig {
    pub codex_bin: PathBuf,
    pub pty_id: Uuid,
    pub sessions_dir: PathBuf,
    pub correlate_sock: PathBuf,
    pub args: Vec<OsString>,
}

pub fn wrapper_path() -> PathBuf {
    let preferred = PathBuf::from(DEFAULT_WRAPPER_PATH);
    if preferred.exists() {
        preferred
    } else {
        PathBuf::from("codex")
    }
}

pub fn parse_launcher_args(args: &[OsString]) -> anyhow::Result<LauncherConfig> {
    let mut codex_bin: Option<PathBuf> = None;
    let mut pty_id: Option<Uuid> = None;
    let mut sessions_dir: Option<PathBuf> = None;
    let mut correlate_sock: Option<PathBuf> = None;
    let mut codex_args = Vec::new();

    let mut i = 0usize;
    while i < args.len() {
        let Some(arg) = args[i].to_str() else {
            return Err(anyhow::anyhow!("launcher arg is not valid utf-8"));
        };
        if arg == "--" {
            codex_args.extend(args[i + 1..].iter().cloned());
            break;
        }
        let next = |idx: usize| -> anyhow::Result<&str> {
            args.get(idx + 1)
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow::anyhow!("missing value for {}", args[idx].to_string_lossy()))
        };
        match arg {
            "--codex-bin" => {
                codex_bin = Some(PathBuf::from(next(i)?));
                i += 2;
            }
            "--pty-id" => {
                pty_id = Some(Uuid::parse_str(next(i)?)?);
                i += 2;
            }
            "--sessions-dir" => {
                sessions_dir = Some(PathBuf::from(next(i)?));
                i += 2;
            }
            "--correlate-sock" => {
                correlate_sock = Some(PathBuf::from(next(i)?));
                i += 2;
            }
            other => {
                return Err(anyhow::anyhow!("unknown launcher arg: {other}"));
            }
        }
    }

    Ok(LauncherConfig {
        codex_bin: codex_bin.ok_or_else(|| anyhow::anyhow!("--codex-bin is required"))?,
        pty_id: pty_id.ok_or_else(|| anyhow::anyhow!("--pty-id is required"))?,
        sessions_dir: sessions_dir.ok_or_else(|| anyhow::anyhow!("--sessions-dir is required"))?,
        correlate_sock: correlate_sock
            .ok_or_else(|| anyhow::anyhow!("--correlate-sock is required"))?,
        args: codex_args,
    })
}

pub async fn run_launcher(cfg: LauncherConfig) -> anyhow::Result<i32> {
    let mut cmd = Command::new(&cfg.codex_bin);
    cmd.args(&cfg.args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(false);

    let mut child = cmd.spawn().map_err(|err| {
        anyhow::anyhow!(
            "failed to spawn codex binary {}: {err}",
            cfg.codex_bin.display()
        )
    })?;
    let root_pid = child
        .id()
        .ok_or_else(|| anyhow::anyhow!("spawned codex process has no pid"))?;

    let mut correlated = false;
    let mut last_observed_session = None;
    let mut ticker = tokio::time::interval(FD_SCAN_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;

        if !correlated {
            if let Some(session_uuid) =
                detect_rollout_session_uuid_in_pid_tree(root_pid, &cfg.sessions_dir)
            {
                match crate::correlate::send_for_agent(
                    &cfg.correlate_sock,
                    cfg.pty_id,
                    session_uuid,
                    "codex",
                )
                .await
                {
                    Ok(()) => correlated = true,
                    Err(err) => {
                        if last_observed_session != Some(session_uuid) {
                            eprintln!(
                                "sulion: failed to correlate codex session {session_uuid}: {err}"
                            );
                            last_observed_session = Some(session_uuid);
                        }
                    }
                }
            }
        }

        if let Some(status) = child.try_wait()? {
            return Ok(exit_code(status));
        }
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    match status.code() {
        Some(code) => code,
        None => status.signal().map(|sig| 128 + sig).unwrap_or(1),
    }
}

pub fn detect_rollout_session_uuid_in_pid_tree(root_pid: u32, sessions_dir: &Path) -> Option<Uuid> {
    let mut seen = HashSet::new();
    let mut stack = vec![root_pid];
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(uuid) = detect_rollout_session_uuid_in_pid(pid, sessions_dir) {
            return Some(uuid);
        }
        stack.extend(read_child_pids(pid));
    }
    None
}

fn detect_rollout_session_uuid_in_pid(pid: u32, sessions_dir: &Path) -> Option<Uuid> {
    let fd_dir = PathBuf::from(format!("/proc/{pid}/fd"));
    let entries = std::fs::read_dir(fd_dir).ok()?;
    for entry in entries.flatten() {
        let Ok(target) = std::fs::read_link(entry.path()) else {
            continue;
        };
        if !target.starts_with(sessions_dir) {
            continue;
        }
        if let Some(uuid) = crate::ingest::parse_codex_session_uuid(&target) {
            return Some(uuid);
        }
    }
    None
}

fn read_child_pids(pid: u32) -> Vec<u32> {
    let children_path = PathBuf::from(format!("/proc/{pid}/task/{pid}/children"));
    let Ok(raw) = std::fs::read_to_string(children_path) else {
        return Vec::new();
    };
    raw.split_whitespace()
        .filter_map(|part| part.parse::<u32>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_launcher_args_requires_expected_flags() {
        let args = vec![
            OsString::from("--codex-bin"),
            OsString::from("/usr/bin/codex"),
            OsString::from("--pty-id"),
            OsString::from("00000000-0000-0000-0000-000000000001"),
            OsString::from("--sessions-dir"),
            OsString::from("/tmp/sessions"),
            OsString::from("--correlate-sock"),
            OsString::from("/tmp/correlate.sock"),
            OsString::from("--"),
            OsString::from("resume"),
            OsString::from("00000000-0000-0000-0000-000000000002"),
        ];
        let parsed = parse_launcher_args(&args).unwrap();
        assert_eq!(parsed.codex_bin, PathBuf::from("/usr/bin/codex"));
        assert_eq!(
            parsed.pty_id,
            Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
        );
        assert_eq!(parsed.sessions_dir, PathBuf::from("/tmp/sessions"));
        assert_eq!(parsed.correlate_sock, PathBuf::from("/tmp/correlate.sock"));
        assert_eq!(
            parsed.args,
            vec![
                OsString::from("resume"),
                OsString::from("00000000-0000-0000-0000-000000000002")
            ]
        );
    }
}
