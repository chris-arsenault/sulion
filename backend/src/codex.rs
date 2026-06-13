use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::process::Command;
use uuid::Uuid;

const FD_SCAN_INTERVAL: Duration = Duration::from_millis(100);
const DESCENDANT_FD_SCAN_GRACE: Duration = Duration::from_millis(500);
const DEFAULT_WRAPPER_PATH: &str = "/opt/sulion/bin/codex";
const LAUNCH_ID_ENV: &str = "SULION_CODEX_LAUNCH_ID";

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
    let launch_id = Uuid::new_v4();
    let mut cmd = Command::new(&cfg.codex_bin);
    cmd.args(&cfg.args)
        .env(LAUNCH_ID_ENV, launch_id.to_string())
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
    let launched_at = Instant::now();
    let mut ticker = tokio::time::interval(FD_SCAN_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;

        if !correlated {
            if let Some(session_uuid) = detect_rollout_session_uuid_in_launched_process(
                root_pid,
                &cfg.sessions_dir,
                launch_id,
                launched_at.elapsed(),
            ) {
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

pub fn detect_rollout_session_uuid_in_launched_process(
    root_pid: u32,
    sessions_dir: &Path,
    launch_id: Uuid,
    elapsed: Duration,
) -> Option<Uuid> {
    if let Some(uuid) = detect_rollout_session_uuid_in_pid(root_pid, sessions_dir) {
        return Some(uuid);
    }

    if elapsed < DESCENDANT_FD_SCAN_GRACE {
        return None;
    }

    for child_pid in child_pids(root_pid) {
        if !process_has_launch_id(child_pid, launch_id) {
            continue;
        }
        if let Some(uuid) = detect_rollout_session_uuid_in_pid(child_pid, sessions_dir) {
            return Some(uuid);
        }
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

fn process_has_launch_id(pid: u32, launch_id: Uuid) -> bool {
    let Ok(raw) = std::fs::read(format!("/proc/{pid}/environ")) else {
        return false;
    };
    let expected = launch_id.to_string();
    raw.split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .any(|arg| {
            arg.strip_prefix(LAUNCH_ID_ENV.as_bytes())
                .and_then(|value| value.strip_prefix(b"="))
                .is_some_and(|value| value == expected.as_bytes())
        })
}

fn child_pids(pid: u32) -> Vec<u32> {
    let tasks_dir = PathBuf::from(format!("/proc/{pid}/task"));
    let mut children = BTreeSet::new();
    let Ok(tasks) = std::fs::read_dir(tasks_dir) else {
        return Vec::new();
    };
    for task in tasks.flatten() {
        let children_path = task.path().join("children");
        let Ok(raw) = std::fs::read_to_string(children_path) else {
            continue;
        };
        for child in raw.split_whitespace() {
            if let Ok(pid) = child.parse() {
                children.insert(pid);
            }
        }
    }
    children.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

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

    #[test]
    fn detects_rollout_fd_opened_by_codex_node_wrapper_child() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let day_dir = sessions_dir.join("2026").join("04").join("19");
        std::fs::create_dir_all(&day_dir).unwrap();
        let launch_id = Uuid::new_v4();

        let session_uuid = Uuid::new_v4();
        let rollout_path =
            day_dir.join(format!("rollout-2026-04-19T01-53-43-{session_uuid}.jsonl"));
        std::fs::write(&rollout_path, "").unwrap();

        let node_path = tmp.path().join("node");
        std::os::unix::fs::symlink("/bin/sh", &node_path).unwrap();
        let codex_script = tmp.path().join("codex");
        std::fs::write(
            &codex_script,
            r#"#!/bin/sh
child_rollout="$1"
sh -c 'exec 4>>"$1"; printf "ready\n"; sleep 5' child "$child_rollout" &
child=$!
wait "$child"
"#,
        )
        .unwrap();
        make_executable(&codex_script);

        let mut child = StdCommand::new(&node_path)
            .arg(&codex_script)
            .arg(&rollout_path)
            .env(LAUNCH_ID_ENV, launch_id.to_string())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        let mut stdout = child.stdout.take().unwrap();
        let mut buf = [0u8; 6];
        std::io::Read::read_exact(&mut stdout, &mut buf).unwrap();

        let mut detected = None;
        for _ in 0..20 {
            detected = detect_rollout_session_uuid_in_launched_process(
                child.id(),
                &sessions_dir,
                launch_id,
                DESCENDANT_FD_SCAN_GRACE,
            );
            if detected.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(detected, Some(session_uuid));

        kill_children(child.id());
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn ignores_rollout_fd_opened_by_marked_grandchild() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let day_dir = sessions_dir.join("2026").join("04").join("19");
        std::fs::create_dir_all(&day_dir).unwrap();
        let launch_id = Uuid::new_v4();

        let session_uuid = Uuid::new_v4();
        let rollout_path =
            day_dir.join(format!("rollout-2026-04-19T01-53-43-{session_uuid}.jsonl"));
        std::fs::write(&rollout_path, "").unwrap();

        let root_script = tmp.path().join("root.sh");
        let middle_script = tmp.path().join("middle.sh");
        let writer_script = tmp.path().join("writer.sh");
        std::fs::write(
            &root_script,
            r#"#!/bin/sh
"$1" "$2" "$3" &
child=$!
wait "$child"
"#,
        )
        .unwrap();
        std::fs::write(
            &middle_script,
            r#"#!/bin/sh
"$1" "$2" &
child=$!
wait "$child"
"#,
        )
        .unwrap();
        std::fs::write(
            &writer_script,
            r#"#!/bin/sh
exec 4>>"$1"
printf "ready\n"
sleep 5
"#,
        )
        .unwrap();
        make_executable(&root_script);
        make_executable(&middle_script);
        make_executable(&writer_script);

        let mut child = StdCommand::new(&root_script)
            .arg(&middle_script)
            .arg(&writer_script)
            .arg(&rollout_path)
            .env(LAUNCH_ID_ENV, launch_id.to_string())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        let mut stdout = child.stdout.take().unwrap();
        let mut buf = [0u8; 6];
        std::io::Read::read_exact(&mut stdout, &mut buf).unwrap();

        assert_eq!(
            detect_rollout_session_uuid_in_launched_process(
                child.id(),
                &sessions_dir,
                launch_id,
                DESCENDANT_FD_SCAN_GRACE,
            ),
            None
        );

        kill_children(child.id());
        let _ = child.kill();
        let _ = child.wait();
    }

    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    fn kill_children(pid: u32) {
        for child_pid in child_pids(pid) {
            kill_children(child_pid);
            unsafe {
                libc::kill(child_pid as i32, libc::SIGKILL);
            }
        }
    }
}
