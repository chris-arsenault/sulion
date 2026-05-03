//! PTY session lifecycle: spawn shells inside allocated pseudo-terminals,
//! supervise their exit, and broadcast their output to subscribers.
//!
//! The manager owns the process tree. Subscribers (WebSocket attach, shadow
//! emulator) consume bytes via a `broadcast` channel and send input via an
//! `mpsc`. A spawn_blocking reader pumps PTY master → broadcast; a
//! spawn_blocking writer pumps mpsc → PTY master.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use portable_pty::{CommandBuilder, MasterPty, PtySize};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use uuid::Uuid;

use crate::db::Pool;
use crate::emulator::ShadowEmulator;

/// Bytes broadcast channel capacity. One slot ~= one PTY read chunk. At
/// 8 KiB/read a 4096-slot buffer holds ~32 MiB of un-drained backlog.
const BROADCAST_CAPACITY: usize = 4096;

/// Size of each PTY read. Larger = fewer syscalls, smaller = lower latency.
const READ_CHUNK: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyState {
    Live,
    Dead,
    Deleted,
    /// Process was running when the backend last stopped and never got
    /// a supervisor signal — we can't resume it, but the row (and its
    /// linked agent session) is still useful for "resume Claude in a
    /// fresh PTY" workflows.
    Orphaned,
}

impl PtyState {
    fn as_str(self) -> &'static str {
        match self {
            PtyState::Live => "live",
            PtyState::Dead => "dead",
            PtyState::Deleted => "deleted",
            PtyState::Orphaned => "orphaned",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "live" => Some(PtyState::Live),
            "dead" => Some(PtyState::Dead),
            "deleted" => Some(PtyState::Deleted),
            "orphaned" => Some(PtyState::Orphaned),
            _ => None,
        }
    }
}

/// On startup, any `live` rows correspond to PTYs whose processes died
/// with the prior backend. Reconcile them to `orphaned`.
pub async fn reconcile_orphans_on_startup(pool: &Pool) -> anyhow::Result<u64> {
    let result = sqlx::query(
        "UPDATE pty_sessions \
         SET state = 'orphaned', \
             ended_at = COALESCE(ended_at, NOW()), \
             agent_runtime_state = CASE \
                 WHEN agent_runtime_state IN ('starting', 'running') THEN 'exited' \
                 ELSE agent_runtime_state \
             END, \
             agent_runtime_ended_at = CASE \
                 WHEN agent_runtime_state IN ('starting', 'running') THEN COALESCE(agent_runtime_ended_at, NOW()) \
                 ELSE agent_runtime_ended_at \
             END \
         WHERE state = 'live'",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

#[derive(Debug, Clone)]
pub struct PtyMetadata {
    pub id: Uuid,
    pub repo: String,
    pub working_dir: PathBuf,
    pub workspace: Option<PtyWorkspaceMetadata>,
    pub state: PtyState,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub exit_code: Option<i32>,
    pub current_session_uuid: Option<Uuid>,
    pub current_session_agent: Option<String>,
    /// MAX(events.timestamp) for the session's current transcript session,
    /// populated by `list()` only.
    pub last_event_at: Option<chrono::DateTime<chrono::Utc>>,
    /// User-facing label; overrides the uuid prefix in the sidebar.
    pub label: Option<String>,
    /// Pinned sessions float to the top of their repo group.
    pub pinned: bool,
    /// Palette-constrained colour tag. See PALETTE in api/routes.rs.
    pub color: Option<String>,
    /// Runtime state for a first-class agent process launched inside this PTY.
    /// Distinct from `current_session_*`, which describes the latest
    /// correlated transcript session.
    pub agent_runtime: AgentRuntimeMetadata,
}

#[derive(Debug, Clone)]
pub struct PtyWorkspaceMetadata {
    pub id: Uuid,
    pub repo_name: String,
    pub kind: String,
    pub path: PathBuf,
    pub branch_name: Option<String>,
    pub base_ref: Option<String>,
    pub base_sha: Option<String>,
    pub merge_target: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentRuntimeMetadata {
    pub agent: Option<String>,
    pub state: String,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub exit_code: Option<i32>,
}

impl Default for AgentRuntimeMetadata {
    fn default() -> Self {
        Self {
            agent: None,
            state: "none".to_string(),
            started_at: None,
            ended_at: None,
            exit_code: None,
        }
    }
}

/// Live, running PTY session. Holds the master PTY and the channels used
/// by subscribers to attach.
pub struct PtySession {
    pub id: Uuid,
    pub repo: String,
    pub working_dir: PathBuf,
    pub workspace: Option<PtyWorkspaceMetadata>,
    /// Broadcast channel of PTY output bytes. Every subscriber (WS attacher)
    /// gets a copy. The shadow emulator is fed directly by the reader task.
    pub output: broadcast::Sender<Vec<u8>>,
    /// Inbound-to-PTY mpsc. Input bytes from WS attachers land here and are
    /// drained by the writer task.
    pub input: mpsc::Sender<Vec<u8>>,
    /// Resize requests. Drained by a small task that calls TIOCSWINSZ.
    pub resize: mpsc::Sender<PtySize>,
    /// Shadow terminal emulator. Fed every byte read from the PTY by the
    /// reader task. Used to render the snapshot-on-attach for WS clients.
    pub emulator: ShadowEmulator,
    /// Process ID of the shell (for signaling). None if already reaped.
    pub pid: Arc<std::sync::Mutex<Option<u32>>>,
}

/// In-memory manager of live PTY sessions, plus a Postgres pool for
/// persistence.
pub struct PtyManager {
    pool: Pool,
    sessions: RwLock<HashMap<Uuid, Arc<PtySession>>>,
}

#[derive(Debug, Clone)]
pub struct SpawnParams {
    pub repo: String,
    pub working_dir: PathBuf,
    pub workspace: Option<PtyWorkspaceMetadata>,
    pub shell: PathBuf,
    pub args: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    pub initial_agent_runtime_agent: Option<String>,
}

impl Default for SpawnParams {
    fn default() -> Self {
        Self {
            repo: String::new(),
            working_dir: PathBuf::from("."),
            workspace: None,
            shell: PathBuf::from("/bin/bash"),
            args: Vec::new(),
            cols: 120,
            rows: 30,
            initial_agent_runtime_agent: None,
        }
    }
}

impl PtyManager {
    pub fn new(pool: Pool) -> Arc<Self> {
        Arc::new(Self {
            pool,
            sessions: RwLock::new(HashMap::new()),
        })
    }

    /// Count of currently-tracked PTY sessions (live + any still in the
    /// map that haven't been reaped). Drives the app-state stats surface.
    pub async fn live_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Spawn a new PTY + shell. Persists a pty_sessions row and starts
    /// reader / writer / supervisor tasks.
    pub async fn spawn(self: &Arc<Self>, params: SpawnParams) -> anyhow::Result<PtyMetadata> {
        let id = Uuid::new_v4();
        let secret_broker_key_path = crate::secret_pty::prepare_pty_credential(id).await?;
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: params.rows,
                cols: params.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow::anyhow!("openpty failed: {e}"))?;

        let mut cmd = CommandBuilder::new(&params.shell);
        for arg in &params.args {
            cmd.arg(arg);
        }
        cmd.cwd(&params.working_dir);
        configure_pty_environment(
            &mut cmd,
            id,
            &params.shell,
            secret_broker_key_path.as_ref(),
            params.workspace.as_ref(),
        );
        if let Ok(term) = std::env::var("TERM") {
            cmd.env("TERM", term);
        } else {
            cmd.env("TERM", "xterm-256color");
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow::anyhow!("spawn shell: {e}"))?;
        // Drop the slave half in the parent; the child owns it now.
        drop(pair.slave);

        let pid = Arc::new(std::sync::Mutex::new(child.process_id()));

        let (out_tx, _) = broadcast::channel::<Vec<u8>>(BROADCAST_CAPACITY);
        let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(64);
        let (resize_tx, resize_rx) = mpsc::channel::<PtySize>(16);
        let emulator = ShadowEmulator::new(params.rows, params.cols);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| anyhow::anyhow!("clone reader: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| anyhow::anyhow!("take writer: {e}"))?;
        // The master must outlive the reader/writer tasks so that the
        // kernel keeps the pty alive. Wrap it in an Arc<Mutex> so the
        // resize task can call resize() on it.
        let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));

        spawn_reader_task(id, reader, out_tx.clone(), emulator.clone());
        spawn_writer_task(id, writer, in_rx);
        spawn_resize_task(id, master.clone(), emulator.clone(), resize_rx);

        let now = chrono::Utc::now();
        let initial_agent_runtime = match params.initial_agent_runtime_agent.clone() {
            Some(agent) => AgentRuntimeMetadata {
                agent: Some(agent),
                state: "starting".to_string(),
                started_at: Some(now),
                ended_at: None,
                exit_code: None,
            },
            None => AgentRuntimeMetadata::default(),
        };
        let meta = PtyMetadata {
            id,
            repo: params.repo.clone(),
            working_dir: params.working_dir.clone(),
            workspace: params.workspace.clone(),
            state: PtyState::Live,
            created_at: now,
            ended_at: None,
            exit_code: None,
            current_session_uuid: None,
            current_session_agent: None,
            last_event_at: None,
            label: None,
            pinned: false,
            color: None,
            agent_runtime: initial_agent_runtime,
        };

        sqlx::query(
            "INSERT INTO pty_sessions \
                (id, repo, working_dir, state, created_at, \
                 agent_runtime_agent, agent_runtime_state, agent_runtime_started_at, workspace_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(meta.id)
        .bind(&meta.repo)
        .bind(meta.working_dir.to_string_lossy().as_ref())
        .bind(meta.state.as_str())
        .bind(meta.created_at)
        .bind(meta.agent_runtime.agent.as_deref())
        .bind(&meta.agent_runtime.state)
        .bind(meta.agent_runtime.started_at)
        .bind(meta.workspace.as_ref().map(|workspace| workspace.id))
        .execute(&self.pool)
        .await?;

        let session = Arc::new(PtySession {
            id,
            repo: meta.repo.clone(),
            working_dir: meta.working_dir.clone(),
            workspace: meta.workspace.clone(),
            output: out_tx,
            input: in_tx,
            resize: resize_tx,
            emulator,
            pid: pid.clone(),
        });
        self.sessions.write().await.insert(id, session);

        spawn_supervisor_task(self.clone(), id, child, pid);

        Ok(meta)
    }

    pub async fn get(&self, id: Uuid) -> Option<Arc<PtySession>> {
        self.sessions.read().await.get(&id).cloned()
    }

    /// Snapshot of sessions plus each one's last-event timestamp and
    /// user-facing metadata (label/pinned/color). Pinned sessions sort
    /// first, then by created_at desc — so the sidebar ordering is
    /// consistent for every client.
    pub async fn list(&self) -> anyhow::Result<Vec<PtyMetadata>> {
        let rows = sqlx::query_as::<_, PtyRowWithActivity>(
            "SELECT ps.id, ps.repo, ps.working_dir, ps.state, ps.created_at, \
             ps.ended_at, ps.exit_code, ps.current_session_uuid, ps.current_session_agent, \
             ps.label, ps.pinned, ps.color, \
             ps.agent_runtime_agent, ps.agent_runtime_state, ps.agent_runtime_started_at, \
             ps.agent_runtime_ended_at, ps.agent_runtime_exit_code, \
             ws.id AS workspace_id, ws.repo_name AS workspace_repo_name, \
             ws.kind AS workspace_kind, ws.path AS workspace_path, \
             ws.branch_name AS workspace_branch_name, ws.base_ref AS workspace_base_ref, \
             ws.base_sha AS workspace_base_sha, ws.merge_target AS workspace_merge_target, \
             (SELECT MAX(e.timestamp) FROM events e \
              WHERE e.session_uuid = ps.current_session_uuid) AS last_event_at \
             FROM pty_sessions ps \
             LEFT JOIN workspaces ws ON ws.id = ps.workspace_id \
             WHERE ps.state <> 'deleted' \
             ORDER BY ps.pinned DESC, ps.created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(PtyRowWithActivity::into_meta)
            .collect())
    }

    /// Update user-facing metadata. Each field is optional; null means
    /// "no change to this field." To clear a label or color, pass
    /// `Some(String::new())` — that value round-trips to NULL in the DB.
    pub async fn update_metadata(
        &self,
        id: Uuid,
        label: Option<Option<String>>,
        pinned: Option<bool>,
        color: Option<Option<String>>,
    ) -> anyhow::Result<()> {
        // Build SET clause dynamically — sqlx doesn't compose cleanly
        // with optional column updates, so we hand-roll the query.
        let mut set_parts: Vec<String> = Vec::new();
        let mut has_change = false;
        if label.is_some() {
            set_parts.push("label = $2".to_string());
            has_change = true;
        }
        if pinned.is_some() {
            set_parts.push(format!("pinned = ${}", if label.is_some() { 3 } else { 2 }));
            has_change = true;
        }
        if color.is_some() {
            let n = 2 + label.is_some() as usize + pinned.is_some() as usize;
            set_parts.push(format!("color = ${n}"));
            has_change = true;
        }
        if !has_change {
            return Ok(());
        }

        let sql = format!(
            "UPDATE pty_sessions SET {} WHERE id = $1",
            set_parts.join(", "),
        );
        let mut query = sqlx::query(&sql).bind(id);
        if let Some(l) = label {
            // empty string → NULL (clear)
            let v = l.filter(|s| !s.is_empty());
            query = query.bind(v);
        }
        if let Some(p) = pinned {
            query = query.bind(p);
        }
        if let Some(c) = color {
            let v = c.filter(|s| !s.is_empty());
            query = query.bind(v);
        }
        query.execute(&self.pool).await?;
        Ok(())
    }

    pub async fn mark_agent_starting(&self, id: Uuid, agent: &str) -> anyhow::Result<()> {
        let result = sqlx::query(
            "UPDATE pty_sessions \
             SET agent_runtime_agent = $2, \
                 agent_runtime_state = 'starting', \
                 agent_runtime_started_at = NOW(), \
                 agent_runtime_ended_at = NULL, \
                 agent_runtime_exit_code = NULL \
             WHERE id = $1 AND state = 'live'",
        )
        .bind(id)
        .bind(agent)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            anyhow::bail!("no live PTY session {id}");
        }
        Ok(())
    }

    pub async fn send_input(&self, id: Uuid, bytes: Vec<u8>) -> anyhow::Result<()> {
        let session = self
            .get(id)
            .await
            .ok_or_else(|| anyhow::anyhow!("no live PTY session {id}"))?;
        session
            .input
            .send(bytes)
            .await
            .map_err(|_| anyhow::anyhow!("PTY input channel closed"))?;
        Ok(())
    }

    /// SIGTERM → grace period → SIGKILL. Marks the DB row deleted.
    pub async fn delete(&self, id: Uuid) -> anyhow::Result<()> {
        let session = self.sessions.write().await.remove(&id);
        if let Some(session) = session {
            let maybe_pid = { *session.pid.lock().unwrap() };
            if let Some(pid) = maybe_pid {
                // SIGTERM first; supervisor will reap. If still alive after
                // 3s, escalate to SIGKILL.
                unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                let alive = wait_for_exit(pid, std::time::Duration::from_secs(3)).await;
                if alive {
                    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                }
            }
        }
        sqlx::query(
            "UPDATE pty_sessions SET state = 'deleted', ended_at = COALESCE(ended_at, NOW()) \
             WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        crate::secret_pty::revoke_pty_credential(id).await;
        Ok(())
    }

    /// Called by the supervisor task when the child process exits.
    async fn mark_dead(&self, id: Uuid, exit_code: Option<i32>) {
        self.sessions.write().await.remove(&id);
        if let Err(err) = sqlx::query(
            "UPDATE pty_sessions \
             SET state = 'dead', \
                 ended_at = NOW(), \
                 exit_code = $2, \
                 agent_runtime_state = CASE \
                     WHEN agent_runtime_state IN ('starting', 'running') THEN 'exited' \
                     ELSE agent_runtime_state \
                 END, \
                 agent_runtime_ended_at = CASE \
                     WHEN agent_runtime_state IN ('starting', 'running') THEN NOW() \
                     ELSE agent_runtime_ended_at \
                 END, \
                 agent_runtime_exit_code = CASE \
                     WHEN agent_runtime_state IN ('starting', 'running') THEN $2 \
                     ELSE agent_runtime_exit_code \
                 END \
             WHERE id = $1 AND state = 'live'",
        )
        .bind(id)
        .bind(exit_code)
        .execute(&self.pool)
        .await
        {
            tracing::error!(%id, %err, "mark_dead failed");
        }
        crate::secret_pty::revoke_pty_credential(id).await;
    }
}

fn configure_pty_environment(
    cmd: &mut CommandBuilder,
    id: Uuid,
    shell: &Path,
    secret_broker_key_path: Option<&PathBuf>,
    workspace: Option<&PtyWorkspaceMetadata>,
) {
    cmd.env_clear();

    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/dev".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "dev".to_string());
    cmd.env("HOME", &home);
    cmd.env("USER", &user);
    cmd.env("LOGNAME", &user);
    cmd.env("SHELL", shell.as_os_str());
    cmd.env("PATH", default_pty_path(&home));

    for key in [
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
        "COLORTERM",
        "RUSTUP_HOME",
        "CARGO_HOME",
        "RUSTUP_TOOLCHAIN",
        "DOTNET_ROOT",
        "DOTNET_CLI_TELEMETRY_OPTOUT",
        "DOTNET_NOLOGO",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        forward_env(cmd, key);
    }

    cmd.env("SULION_PTY_ID", id.to_string());
    if let Some(workspace) = workspace {
        cmd.env("SULION_REPO_NAME", &workspace.repo_name);
        cmd.env("SULION_WORKSPACE_ID", workspace.id.to_string());
        cmd.env("SULION_WORKSPACE_KIND", &workspace.kind);
        cmd.env(
            "SULION_WORKSPACE_PATH",
            workspace.path.to_string_lossy().as_ref(),
        );
        cmd.env(
            "SULION_CANONICAL_REPO",
            std::env::var("SULION_REPOS_ROOT")
                .map(|root| PathBuf::from(root).join(&workspace.repo_name))
                .unwrap_or_else(|_| PathBuf::from("/home/dev/repos").join(&workspace.repo_name))
                .to_string_lossy()
                .as_ref(),
        );
        if let Some(branch) = &workspace.branch_name {
            cmd.env("SULION_BRANCH", branch);
        }
        if let Some(base_ref) = &workspace.base_ref {
            cmd.env("SULION_BASE_REF", base_ref);
        }
        if let Some(base_sha) = &workspace.base_sha {
            cmd.env("SULION_BASE_SHA", base_sha);
        }
        if let Some(merge_target) = &workspace.merge_target {
            cmd.env("SULION_MERGE_TARGET", merge_target);
        }
    }
    cmd.env(
        "SULION_CORRELATE_SOCK",
        std::env::var("SULION_CORRELATE_SOCK")
            .unwrap_or_else(|_| "/run/sulion/correlate.sock".to_string()),
    );
    for key in [
        "SULION_CLAUDE_PROJECTS",
        "SULION_CODEX_SESSIONS",
        "SULION_SECRET_BROKER_URL",
        "SULION_AWS_SECRET_ID",
        "SULION_RUNNER_URL",
    ] {
        forward_env(cmd, key);
    }
    if let Some(path) = secret_broker_key_path {
        cmd.env(
            "SULION_SECRET_BROKER_KEY_PATH",
            path.to_string_lossy().as_ref(),
        );
    }
}

fn default_pty_path(home: &str) -> String {
    let base = std::env::var("PATH")
        .unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin:/usr/local/sbin:/usr/sbin:/sbin".into());
    format!("{home}/.local/bin:/opt/sulion/bin:{base}")
}

fn forward_env(cmd: &mut CommandBuilder, key: &str) {
    if let Ok(value) = std::env::var(key) {
        cmd.env(key, value);
    }
}

#[derive(sqlx::FromRow)]
struct PtyRow {
    id: Uuid,
    repo: String,
    working_dir: String,
    workspace_id: Option<Uuid>,
    workspace_repo_name: Option<String>,
    workspace_kind: Option<String>,
    workspace_path: Option<String>,
    workspace_branch_name: Option<String>,
    workspace_base_ref: Option<String>,
    workspace_base_sha: Option<String>,
    workspace_merge_target: Option<String>,
    state: String,
    created_at: chrono::DateTime<chrono::Utc>,
    ended_at: Option<chrono::DateTime<chrono::Utc>>,
    exit_code: Option<i32>,
    current_session_uuid: Option<Uuid>,
    current_session_agent: Option<String>,
    agent_runtime_agent: Option<String>,
    agent_runtime_state: String,
    agent_runtime_started_at: Option<chrono::DateTime<chrono::Utc>>,
    agent_runtime_ended_at: Option<chrono::DateTime<chrono::Utc>>,
    agent_runtime_exit_code: Option<i32>,
}

impl PtyRow {
    fn into_meta(self) -> PtyMetadata {
        let workspace = self.workspace_meta();
        PtyMetadata {
            id: self.id,
            repo: self.repo,
            working_dir: PathBuf::from(self.working_dir),
            workspace,
            state: PtyState::parse(&self.state).unwrap_or(PtyState::Dead),
            created_at: self.created_at,
            ended_at: self.ended_at,
            exit_code: self.exit_code,
            current_session_uuid: self.current_session_uuid,
            current_session_agent: self.current_session_agent,
            last_event_at: None,
            label: None,
            pinned: false,
            color: None,
            agent_runtime: AgentRuntimeMetadata {
                agent: self.agent_runtime_agent,
                state: self.agent_runtime_state,
                started_at: self.agent_runtime_started_at,
                ended_at: self.agent_runtime_ended_at,
                exit_code: self.agent_runtime_exit_code,
            },
        }
    }

    fn workspace_meta(&self) -> Option<PtyWorkspaceMetadata> {
        Some(PtyWorkspaceMetadata {
            id: self.workspace_id?,
            repo_name: self.workspace_repo_name.clone()?,
            kind: self.workspace_kind.clone()?,
            path: PathBuf::from(self.workspace_path.clone()?),
            branch_name: self.workspace_branch_name.clone(),
            base_ref: self.workspace_base_ref.clone(),
            base_sha: self.workspace_base_sha.clone(),
            merge_target: self.workspace_merge_target.clone(),
        })
    }
}

/// Extended row used by `list()` — includes activity timestamp plus
/// user metadata (label/pinned/color).
#[derive(sqlx::FromRow)]
struct PtyRowWithActivity {
    id: Uuid,
    repo: String,
    working_dir: String,
    workspace_id: Option<Uuid>,
    workspace_repo_name: Option<String>,
    workspace_kind: Option<String>,
    workspace_path: Option<String>,
    workspace_branch_name: Option<String>,
    workspace_base_ref: Option<String>,
    workspace_base_sha: Option<String>,
    workspace_merge_target: Option<String>,
    state: String,
    created_at: chrono::DateTime<chrono::Utc>,
    ended_at: Option<chrono::DateTime<chrono::Utc>>,
    exit_code: Option<i32>,
    current_session_uuid: Option<Uuid>,
    current_session_agent: Option<String>,
    label: Option<String>,
    pinned: bool,
    color: Option<String>,
    agent_runtime_agent: Option<String>,
    agent_runtime_state: String,
    agent_runtime_started_at: Option<chrono::DateTime<chrono::Utc>>,
    agent_runtime_ended_at: Option<chrono::DateTime<chrono::Utc>>,
    agent_runtime_exit_code: Option<i32>,
    last_event_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl PtyRowWithActivity {
    fn into_meta(self) -> PtyMetadata {
        let workspace = self.workspace_meta();
        PtyMetadata {
            id: self.id,
            repo: self.repo,
            working_dir: PathBuf::from(self.working_dir),
            workspace,
            state: PtyState::parse(&self.state).unwrap_or(PtyState::Dead),
            created_at: self.created_at,
            ended_at: self.ended_at,
            exit_code: self.exit_code,
            current_session_uuid: self.current_session_uuid,
            current_session_agent: self.current_session_agent,
            last_event_at: self.last_event_at,
            label: self.label,
            pinned: self.pinned,
            color: self.color,
            agent_runtime: AgentRuntimeMetadata {
                agent: self.agent_runtime_agent,
                state: self.agent_runtime_state,
                started_at: self.agent_runtime_started_at,
                ended_at: self.agent_runtime_ended_at,
                exit_code: self.agent_runtime_exit_code,
            },
        }
    }

    fn workspace_meta(&self) -> Option<PtyWorkspaceMetadata> {
        Some(PtyWorkspaceMetadata {
            id: self.workspace_id?,
            repo_name: self.workspace_repo_name.clone()?,
            kind: self.workspace_kind.clone()?,
            path: PathBuf::from(self.workspace_path.clone()?),
            branch_name: self.workspace_branch_name.clone(),
            base_ref: self.workspace_base_ref.clone(),
            base_sha: self.workspace_base_sha.clone(),
            merge_target: self.workspace_merge_target.clone(),
        })
    }
}

// ─── task spawners ───────────────────────────────────────────────────────

fn spawn_reader_task(
    id: Uuid,
    mut reader: Box<dyn Read + Send>,
    tx: broadcast::Sender<Vec<u8>>,
    emulator: ShadowEmulator,
) {
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; READ_CHUNK];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF: child closed its end of the PTY
                Ok(n) => {
                    let chunk = &buf[..n];
                    // Feed the shadow emulator unconditionally so snapshot-on-attach
                    // stays current even when no clients are subscribed.
                    emulator.process(chunk);
                    let _ = tx.send(chunk.to_vec());
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    tracing::debug!(%id, err = %e, "pty read ended");
                    break;
                }
            }
        }
    });
}

fn spawn_writer_task(id: Uuid, mut writer: Box<dyn Write + Send>, mut rx: mpsc::Receiver<Vec<u8>>) {
    tokio::task::spawn_blocking(move || {
        while let Some(bytes) = rx.blocking_recv() {
            if let Err(err) = writer.write_all(&bytes) {
                tracing::debug!(%id, %err, "pty write failed");
                break;
            }
            let _ = writer.flush();
        }
    });
}

fn spawn_resize_task(
    id: Uuid,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    emulator: ShadowEmulator,
    mut rx: mpsc::Receiver<PtySize>,
) {
    tokio::spawn(async move {
        while let Some(size) = rx.recv().await {
            {
                let m = master.lock().await;
                if let Err(err) = m.resize(size) {
                    tracing::warn!(%id, %err, "pty resize failed");
                }
            }
            // Keep the emulator dimensions in sync so the next snapshot
            // is correctly shaped.
            emulator.resize(size.rows, size.cols);
        }
    });
}

fn spawn_supervisor_task(
    manager: Arc<PtyManager>,
    id: Uuid,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    pid: Arc<std::sync::Mutex<Option<u32>>>,
) {
    tokio::task::spawn_blocking(move || {
        let status = child.wait();
        // ExitCode is u32; signal-terminated processes encode the signal
        // differently across impls — we just cast to i32 for storage.
        let exit_code = status.ok().map(|s| s.exit_code() as i32);
        *pid.lock().unwrap() = None;
        let manager = manager.clone();
        tokio::runtime::Handle::current().spawn(async move {
            manager.mark_dead(id, exit_code).await;
        });
    });
}

async fn wait_for_exit(pid: u32, timeout: std::time::Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        // Sending signal 0 probes whether the pid is alive without harming it.
        let rc = unsafe { libc::kill(pid as i32, 0) };
        if rc != 0 {
            // errno ESRCH = no such process → already exited
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // Still alive at deadline.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Helper: read the most recent PtyMetadata for an id directly from DB.
/// Used by tests to assert final state.
pub async fn read_meta(pool: &Pool, id: Uuid) -> anyhow::Result<Option<PtyMetadata>> {
    let row = sqlx::query_as::<_, PtyRow>(
        "SELECT ps.id, ps.repo, ps.working_dir, ps.state, ps.created_at, ps.ended_at, ps.exit_code, \
         ps.current_session_uuid, ps.current_session_agent, \
         ps.agent_runtime_agent, ps.agent_runtime_state, ps.agent_runtime_started_at, \
         ps.agent_runtime_ended_at, ps.agent_runtime_exit_code, \
         ws.id AS workspace_id, ws.repo_name AS workspace_repo_name, \
         ws.kind AS workspace_kind, ws.path AS workspace_path, \
         ws.branch_name AS workspace_branch_name, ws.base_ref AS workspace_base_ref, \
         ws.base_sha AS workspace_base_sha, ws.merge_target AS workspace_merge_target \
         FROM pty_sessions ps \
         LEFT JOIN workspaces ws ON ws.id = ps.workspace_id \
         WHERE ps.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(PtyRow::into_meta))
}

/// Path-agnostic shell probe used by tests/docs.
pub fn default_shell() -> PathBuf {
    if Path::new("/bin/bash").exists() {
        PathBuf::from("/bin/bash")
    } else {
        PathBuf::from("/bin/sh")
    }
}
