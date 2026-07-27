//! Unix-socket listener for agent session correlation and PTY-scoped
//! agent runtime state.
//!
//! The backend spawns shells with `SULION_PTY_ID=<uuid>` in their
//! environment. When Claude Code starts a new session in that shell, its
//! `SessionStart` hook reads the env var and posts a single JSON line to
//! this socket:
//!
//! ```json
//! {"pty_id": "<uuid>", "session_uuid": "<uuid>", "agent": "claude-code"}
//! ```
//!
//! Older Claude hooks still send `claude_session_uuid`; we accept that as
//! an alias and default the agent to `claude-code`.
//!
//! The backend upserts the session row, sets its `pty_session_id`
//! to the PTY, and updates `pty_sessions.current_session_uuid` /
//! `current_session_agent` so the UI can show "this PTY is running
//! agent session X."

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use uuid::Uuid;

use crate::db::Pool;

const CORRELATE_IO_TIMEOUT: Duration = Duration::from_millis(750);
const CONTROL_IO_TIMEOUT: Duration = Duration::from_secs(5);
const RUNTIME_UPDATE_RETRY_DELAY: Duration = Duration::from_millis(50);
const RUNTIME_UPDATE_MAX_ATTEMPTS: usize = 20;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SocketMsg {
    Control(ControlMsg),
    Runtime(RuntimeMsg),
    Correlate(CorrelateMsg),
}

#[derive(Debug, Deserialize)]
pub struct CorrelateMsg {
    pub pty_id: Uuid,
    #[serde(alias = "claude_session_uuid")]
    pub session_uuid: Uuid,
    #[serde(default = "default_agent")]
    pub agent: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RuntimeMsg {
    pub pty_id: Uuid,
    pub agent: String,
    pub event: RuntimeEvent,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ControlMsg {
    pub pty_id: Uuid,
    pub control: ControlRequest,
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ControlRequest {
    PlanStart {
        title: String,
        summary: String,
        phases: Vec<crate::plans::NewPhase>,
        all_pending: bool,
    },
    PlanCurrent,
    PlanList {
        include_closed: bool,
    },
    PlanShow {
        plan_id: Option<Uuid>,
    },
    PlanUpdate {
        plan_id: Option<Uuid>,
        input: crate::plans::UpdatePlanInput,
    },
    PlanAddPhase {
        plan_id: Option<Uuid>,
        phase: crate::plans::NewPhase,
    },
    PlanUpdatePhase {
        plan_id: Option<Uuid>,
        phase_reference: String,
        input: crate::plans::UpdatePhaseInput,
    },
    PlanAttach {
        plan_id: Uuid,
    },
    PlanDetach {
        plan_id: Option<Uuid>,
    },
    PlanHistory {
        plan_id: Option<Uuid>,
    },
    ActivitySet {
        state: crate::activity::ActivityState,
        summary: Option<String>,
        reason: Option<String>,
    },
    ActivityClear,
    ActivityGet,
    /// Agent-chosen terminal name. `None` clears it. Complements the
    /// user's label — never overwrites it.
    SessionNameSet {
        name: Option<String>,
    },
    SessionNameGet,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ControlResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEvent {
    Running,
    Exited,
}

fn default_agent() -> String {
    "claude-code".to_string()
}

/// Bind the socket and run an accept loop. The socket file is removed
/// if it already exists (stale from a crashed prior instance).
pub async fn run(pool: Pool, sock_path: PathBuf) -> anyhow::Result<()> {
    if sock_path.exists() {
        let _ = std::fs::remove_file(&sock_path);
    }
    if let Some(parent) = sock_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let listener = UnixListener::bind(&sock_path)?;
    tracing::info!(path = %sock_path.display(), "correlate socket listening");

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let pool = pool.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_conn(&pool, stream).await {
                        tracing::warn!(%err, "correlate: handle_conn error");
                    }
                });
            }
            Err(err) => {
                tracing::warn!(%err, "correlate: accept error");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle_conn(pool: &Pool, stream: UnixStream) -> anyhow::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let bytes_read = tokio::time::timeout(CORRELATE_IO_TIMEOUT, reader.read_line(&mut line))
        .await
        .map_err(|_| timeout_error("waiting for correlate payload"))??;
    if bytes_read == 0 {
        return Ok(());
    }
    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }
    let msg: SocketMsg = serde_json::from_str(line)?;
    let response = match msg {
        SocketMsg::Runtime(msg) => {
            apply_runtime(pool, &msg).await?;
            b"ok\n".to_vec()
        }
        SocketMsg::Correlate(msg) => {
            apply(pool, &msg).await?;
            b"ok\n".to_vec()
        }
        SocketMsg::Control(msg) => {
            let response = match dispatch_control(pool, msg).await {
                Ok(data) => ControlResponse {
                    ok: true,
                    data: Some(data),
                    error: None,
                },
                Err(err) => ControlResponse {
                    ok: false,
                    data: None,
                    error: Some(err.to_string()),
                },
            };
            let mut encoded = serde_json::to_vec(&response)?;
            encoded.push(b'\n');
            encoded
        }
    };

    let _ = tokio::time::timeout(CORRELATE_IO_TIMEOUT, reader.get_mut().write_all(&response)).await;
    Ok(())
}

async fn dispatch_control(pool: &Pool, msg: ControlMsg) -> anyhow::Result<Value> {
    use crate::plans;

    let (actor, repo_name) = plans::PlanActor::for_pty(pool, msg.pty_id).await?;
    match msg.control {
        ControlRequest::PlanStart {
            title,
            summary,
            phases,
            all_pending,
        } => Ok(serde_json::to_value(
            plans::create(
                pool,
                plans::CreatePlanInput {
                    repo_name,
                    title,
                    summary,
                    phases,
                    all_pending,
                    attach_current_pty: true,
                },
                &actor,
            )
            .await?,
        )?),
        ControlRequest::PlanCurrent => Ok(serde_json::to_value(
            plans::current_for_pty(pool, msg.pty_id).await?,
        )?),
        ControlRequest::PlanList { include_closed } => Ok(serde_json::to_value(
            plans::list_for_repo(pool, &repo_name, include_closed).await?,
        )?),
        ControlRequest::PlanShow { plan_id } => {
            let id = plans::resolve_plan_id(pool, msg.pty_id, plan_id, &repo_name).await?;
            Ok(serde_json::to_value(plans::get(pool, id).await?)?)
        }
        ControlRequest::PlanUpdate { plan_id, input } => {
            let id = plans::resolve_plan_id(pool, msg.pty_id, plan_id, &repo_name).await?;
            Ok(serde_json::to_value(
                plans::update(pool, id, input, &actor).await?,
            )?)
        }
        ControlRequest::PlanAddPhase { plan_id, phase } => {
            let id = plans::resolve_plan_id(pool, msg.pty_id, plan_id, &repo_name).await?;
            Ok(serde_json::to_value(
                plans::add_phase(pool, id, phase, &actor).await?,
            )?)
        }
        ControlRequest::PlanUpdatePhase {
            plan_id,
            phase_reference,
            input,
        } => {
            let id = plans::resolve_plan_id(pool, msg.pty_id, plan_id, &repo_name).await?;
            let phase_id = plans::resolve_phase_id(pool, id, &phase_reference).await?;
            Ok(serde_json::to_value(
                plans::update_phase(pool, id, phase_id, input, &actor).await?,
            )?)
        }
        ControlRequest::PlanAttach { plan_id } => Ok(serde_json::to_value(
            plans::attach(pool, plan_id, msg.pty_id, &actor).await?,
        )?),
        ControlRequest::PlanDetach { plan_id } => {
            let id = plans::resolve_plan_id(pool, msg.pty_id, plan_id, &repo_name).await?;
            Ok(serde_json::to_value(
                plans::detach(pool, id, msg.pty_id, &actor).await?,
            )?)
        }
        ControlRequest::PlanHistory { plan_id } => {
            let id = plans::resolve_plan_id(pool, msg.pty_id, plan_id, &repo_name).await?;
            Ok(serde_json::to_value(plans::events(pool, id).await?)?)
        }
        ControlRequest::ActivitySet {
            state,
            summary,
            reason,
        } => Ok(serde_json::to_value(
            crate::activity::set(
                pool,
                msg.pty_id,
                state,
                summary.as_deref(),
                reason.as_deref(),
                "agent",
                "explicit",
            )
            .await?,
        )?),
        ControlRequest::ActivityClear => {
            crate::activity::clear(pool, msg.pty_id).await?;
            Ok(serde_json::json!({ "cleared": true }))
        }
        ControlRequest::ActivityGet => {
            let activity: Option<crate::activity::ActivityRecord> = sqlx::query_as(
                "SELECT pty_session_id, state, summary, reason, source, confidence, updated_at \
                   FROM session_activity_state WHERE pty_session_id = $1",
            )
            .bind(msg.pty_id)
            .fetch_optional(pool)
            .await?;
            Ok(serde_json::to_value(activity)?)
        }
        ControlRequest::SessionNameSet { name } => {
            let cleaned = match name.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(value) if value.chars().count() > 100 => {
                    anyhow::bail!("agent name must be 100 characters or fewer")
                }
                Some(value) => Some(value.to_string()),
            };
            sqlx::query("UPDATE pty_sessions SET agent_label = $2 WHERE id = $1")
                .bind(msg.pty_id)
                .bind(cleaned.as_deref())
                .execute(pool)
                .await?;
            Ok(serde_json::json!({ "agent_label": cleaned }))
        }
        ControlRequest::SessionNameGet => {
            let label: Option<String> =
                sqlx::query_scalar("SELECT agent_label FROM pty_sessions WHERE id = $1")
                    .bind(msg.pty_id)
                    .fetch_optional(pool)
                    .await?
                    .flatten();
            Ok(serde_json::json!({ "agent_label": label }))
        }
    }
}

/// Apply a correlation to the database. Idempotent and race-tolerant:
/// the ingester or the hook might arrive first; both upsert.
pub async fn apply(pool: &Pool, msg: &CorrelateMsg) -> anyhow::Result<()> {
    // Upsert agent session with the pty linkage.
    sqlx::query(
        "INSERT INTO claude_sessions (session_uuid, agent, pty_session_id) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (session_uuid) DO UPDATE \
           SET agent = EXCLUDED.agent, \
               pty_session_id = EXCLUDED.pty_session_id",
    )
    .bind(msg.session_uuid)
    .bind(&msg.agent)
    .bind(msg.pty_id)
    .execute(pool)
    .await?;

    // Point the PTY row at this now-current agent session. Keep the
    // legacy Claude-specific pointer populated only for Claude rows.
    sqlx::query(
        "UPDATE pty_sessions \
         SET current_session_uuid = $2, \
             current_session_agent = $3, \
             current_claude_session_uuid = CASE \
                 WHEN $3 = 'claude-code' THEN $2 \
                 ELSE NULL \
             END \
         WHERE id = $1",
    )
    .bind(msg.pty_id)
    .bind(msg.session_uuid)
    .bind(&msg.agent)
    .execute(pool)
    .await?;

    tracing::info!(
        pty = %msg.pty_id,
        session = %msg.session_uuid,
        agent = %msg.agent,
        "correlation recorded",
    );
    Ok(())
}

/// Apply a PTY-scoped agent runtime transition. This intentionally does
/// not carry transcript session ids: it is the process lifecycle for the
/// first-class agent binary running inside the PTY.
pub async fn apply_runtime(pool: &Pool, msg: &RuntimeMsg) -> anyhow::Result<()> {
    match msg.event {
        RuntimeEvent::Running => {
            let mut rows_affected = 0;
            for attempt in 0..RUNTIME_UPDATE_MAX_ATTEMPTS {
                let result = sqlx::query(
                    "UPDATE pty_sessions \
                     SET agent_runtime_agent = $2, \
                         agent_runtime_state = 'running', \
                         agent_runtime_started_at = COALESCE(agent_runtime_started_at, NOW()), \
                         agent_runtime_ended_at = NULL, \
                         agent_runtime_exit_code = NULL \
                     WHERE id = $1 AND state = 'live'",
                )
                .bind(msg.pty_id)
                .bind(&msg.agent)
                .execute(pool)
                .await?;
                rows_affected = result.rows_affected();
                if rows_affected > 0 {
                    break;
                }
                if attempt + 1 < RUNTIME_UPDATE_MAX_ATTEMPTS {
                    tokio::time::sleep(RUNTIME_UPDATE_RETRY_DELAY).await;
                }
            }
            if rows_affected == 0 {
                tracing::warn!(
                    pty = %msg.pty_id,
                    agent = %msg.agent,
                    "agent runtime running event matched no live PTY",
                );
            }
            if rows_affected > 0 {
                crate::activity::set(
                    pool,
                    msg.pty_id,
                    crate::activity::ActivityState::Unknown,
                    None,
                    None,
                    "launcher",
                    "unknown",
                )
                .await?;
            }
        }
        RuntimeEvent::Exited => {
            sqlx::query(
                "UPDATE pty_sessions \
                 SET agent_runtime_agent = COALESCE(agent_runtime_agent, $2), \
                     agent_runtime_state = 'exited', \
                     agent_runtime_ended_at = NOW(), \
                     agent_runtime_exit_code = $3 \
                 WHERE id = $1 \
                   AND agent_runtime_state IN ('starting', 'running') \
                   AND (agent_runtime_agent IS NULL OR agent_runtime_agent = $2)",
            )
            .bind(msg.pty_id)
            .bind(&msg.agent)
            .bind(msg.exit_code)
            .execute(pool)
            .await?;
            crate::activity::clear(pool, msg.pty_id).await?;
        }
    }

    tracing::info!(
        pty = %msg.pty_id,
        agent = %msg.agent,
        event = ?msg.event,
        exit_code = ?msg.exit_code,
        "agent runtime recorded",
    );
    Ok(())
}

/// Helper used by the hook script on platforms where it's more ergonomic
/// to spawn a small correlator binary than a shell script. Not used by the
/// default hook but useful for diagnostics. Writes one JSON line to the
/// given socket and reads the ACK.
#[allow(dead_code)]
pub fn send_blocking(sock: &Path, pty_id: Uuid, session_uuid: Uuid) -> std::io::Result<()> {
    send_blocking_for_agent(sock, pty_id, session_uuid, "claude-code")
}

pub async fn send_for_agent(
    sock: &Path,
    pty_id: Uuid,
    session_uuid: Uuid,
    agent: &str,
) -> std::io::Result<()> {
    let mut s = tokio::time::timeout(CORRELATE_IO_TIMEOUT, UnixStream::connect(sock))
        .await
        .map_err(|_| timeout_error("connecting to correlate socket"))??;
    let line = correlate_msg_line(pty_id, session_uuid, agent);
    tokio::time::timeout(CORRELATE_IO_TIMEOUT, s.write_all(line.as_bytes()))
        .await
        .map_err(|_| timeout_error("writing correlate payload"))??;
    let mut ack = [0u8; 4];
    match tokio::time::timeout(CORRELATE_IO_TIMEOUT, s.read(&mut ack)).await {
        Ok(Ok(0)) => Err(unexpected_ack_eof()),
        Ok(Ok(_)) => Ok(()),
        Ok(Err(err)) => Err(err),
        Err(_) => Err(timeout_error("waiting for correlate ack")),
    }
}

pub async fn send_agent_runtime(
    sock: &Path,
    pty_id: Uuid,
    agent: &str,
    event: RuntimeEvent,
    exit_code: Option<i32>,
) -> std::io::Result<()> {
    let mut s = tokio::time::timeout(CORRELATE_IO_TIMEOUT, UnixStream::connect(sock))
        .await
        .map_err(|_| timeout_error("connecting to correlate socket"))??;
    let line = runtime_msg_line(pty_id, agent, event, exit_code);
    tokio::time::timeout(CORRELATE_IO_TIMEOUT, s.write_all(line.as_bytes()))
        .await
        .map_err(|_| timeout_error("writing runtime payload"))??;
    let mut ack = [0u8; 4];
    match tokio::time::timeout(CORRELATE_IO_TIMEOUT, s.read(&mut ack)).await {
        Ok(Ok(0)) => Err(unexpected_ack_eof()),
        Ok(Ok(_)) => Ok(()),
        Ok(Err(err)) => Err(err),
        Err(_) => Err(timeout_error("waiting for runtime ack")),
    }
}

pub async fn send_control(
    sock: &Path,
    pty_id: Uuid,
    control: ControlRequest,
) -> std::io::Result<ControlResponse> {
    let mut stream = tokio::time::timeout(CONTROL_IO_TIMEOUT, UnixStream::connect(sock))
        .await
        .map_err(|_| control_timeout_error("connecting to control socket"))??;
    let payload = serde_json::to_vec(&ControlMsg { pty_id, control })
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    tokio::time::timeout(CONTROL_IO_TIMEOUT, stream.write_all(&payload))
        .await
        .map_err(|_| control_timeout_error("writing control payload"))??;
    tokio::time::timeout(CONTROL_IO_TIMEOUT, stream.write_all(b"\n"))
        .await
        .map_err(|_| control_timeout_error("terminating control payload"))??;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let read = tokio::time::timeout(CONTROL_IO_TIMEOUT, reader.read_line(&mut line))
        .await
        .map_err(|_| control_timeout_error("waiting for control response"))??;
    if read == 0 {
        return Err(unexpected_ack_eof());
    }
    serde_json::from_str(&line).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

pub fn send_blocking_for_agent(
    sock: &Path,
    pty_id: Uuid,
    session_uuid: Uuid,
    agent: &str,
) -> std::io::Result<()> {
    use std::io::{Read, Write};
    let mut s = std::os::unix::net::UnixStream::connect(sock)?;
    s.set_write_timeout(Some(CORRELATE_IO_TIMEOUT))?;
    s.set_read_timeout(Some(CORRELATE_IO_TIMEOUT))?;
    let line = correlate_msg_line(pty_id, session_uuid, agent);
    s.write_all(line.as_bytes())?;
    s.flush()?;
    let mut ack = [0u8; 4];
    match s.read(&mut ack) {
        Ok(0) => Err(unexpected_ack_eof()),
        Ok(_) => Ok(()),
        Err(err) => Err(err),
    }
}

fn correlate_msg_line(pty_id: Uuid, session_uuid: Uuid, agent: &str) -> String {
    format!(
        "{{\"pty_id\":\"{pty_id}\",\"session_uuid\":\"{session_uuid}\",\"agent\":\"{agent}\"}}\n"
    )
}

fn runtime_msg_line(
    pty_id: Uuid,
    agent: &str,
    event: RuntimeEvent,
    exit_code: Option<i32>,
) -> String {
    let msg = RuntimeMsg {
        pty_id,
        agent: agent.to_string(),
        event,
        exit_code,
    };
    format!(
        "{}\n",
        serde_json::to_string(&msg).expect("serialize runtime msg")
    )
}

fn timeout_error(phase: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!("timed out {phase} after {:?}", CORRELATE_IO_TIMEOUT),
    )
}

fn control_timeout_error(phase: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!("timed out {phase} after {CONTROL_IO_TIMEOUT:?}"),
    )
}

fn unexpected_ack_eof() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "correlate socket closed before ACK",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_correlate_msg() {
        let pty = Uuid::new_v4();
        let session = Uuid::new_v4();
        let json = format!(r#"{{"pty_id":"{pty}","claude_session_uuid":"{session}"}}"#);
        let parsed: CorrelateMsg = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.pty_id, pty);
        assert_eq!(parsed.session_uuid, session);
        assert_eq!(parsed.agent, "claude-code");
    }

    #[test]
    fn parse_runtime_msg() {
        let pty = Uuid::new_v4();
        let json = format!(r#"{{"pty_id":"{pty}","agent":"codex","event":"running"}}"#);
        let parsed: SocketMsg = serde_json::from_str(&json).unwrap();
        match parsed {
            SocketMsg::Runtime(msg) => {
                assert_eq!(msg.pty_id, pty);
                assert_eq!(msg.agent, "codex");
                assert!(matches!(msg.event, RuntimeEvent::Running));
                assert_eq!(msg.exit_code, None);
            }
            SocketMsg::Correlate(_) => panic!("parsed runtime as correlate"),
            SocketMsg::Control(_) => panic!("parsed runtime as control"),
        }
    }

    #[test]
    fn parse_control_msg() {
        let raw = r#"{"pty_id":"00000000-0000-0000-0000-000000000001","control":{"command":"activity_set","state":"working","summary":"schema","reason":null}}"#;
        let msg: SocketMsg = serde_json::from_str(raw).unwrap();
        assert!(matches!(
            msg,
            SocketMsg::Control(ControlMsg {
                control: ControlRequest::ActivitySet {
                    state: crate::activity::ActivityState::Working,
                    ..
                },
                ..
            })
        ));
    }
}
