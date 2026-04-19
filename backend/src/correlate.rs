//! Unix-socket listener for SessionStart correlation.
//!
//! The backend spawns shells with `SHUTTLECRAFT_PTY_ID=<uuid>` in their
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

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use uuid::Uuid;

use crate::db::Pool;

#[derive(Debug, Deserialize)]
pub struct CorrelateMsg {
    pub pty_id: Uuid,
    #[serde(alias = "claude_session_uuid")]
    pub session_uuid: Uuid,
    #[serde(default = "default_agent")]
    pub agent: String,
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
    reader.read_line(&mut line).await?;
    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }
    let msg: CorrelateMsg = serde_json::from_str(line)?;
    apply(pool, &msg).await?;

    // Tiny ACK so clients that care can block until we've committed.
    let _ = reader.get_mut().write_all(b"ok\n").await;
    Ok(())
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

/// Helper used by the hook script on platforms where it's more ergonomic
/// to spawn a small correlator binary than a shell script. Not used by the
/// default hook but useful for diagnostics. Writes one JSON line to the
/// given socket and reads the ACK.
#[allow(dead_code)]
pub fn send_blocking(sock: &Path, pty_id: Uuid, session_uuid: Uuid) -> std::io::Result<()> {
    send_blocking_for_agent(sock, pty_id, session_uuid, "claude-code")
}

pub fn send_blocking_for_agent(
    sock: &Path,
    pty_id: Uuid,
    session_uuid: Uuid,
    agent: &str,
) -> std::io::Result<()> {
    use std::io::{Read, Write};
    let mut s = std::os::unix::net::UnixStream::connect(sock)?;
    let line = format!(
        "{{\"pty_id\":\"{pty_id}\",\"session_uuid\":\"{session_uuid}\",\"agent\":\"{agent}\"}}\n"
    );
    s.write_all(line.as_bytes())?;
    s.flush()?;
    let mut ack = [0u8; 4];
    let _ = s.read(&mut ack);
    Ok(())
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
}
