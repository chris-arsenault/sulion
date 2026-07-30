//! PTY session bookkeeping: rows, metadata, and the node-side handles that
//! drive shells hosted in the devenv server.
//!
//! The manager owns no process tree. Shells live in the devenv server
//! (`crate::devenv::server`), reached through the link
//! (`crate::devenv::link`); this module keeps the Postgres records and the
//! per-session handles the rest of the node subscribes to. A manager built
//! without a link (the control plane) can only do record-keeping.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, RwLock};
use uuid::Uuid;

use crate::db::Pool;
use crate::devenv::link::{DevenvLink, LinkEvent};

pub mod host;

use host::HostSpawnSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Node-side handle to a live session hosted in the devenv server.
pub struct PtySession {
    pub id: Uuid,
    pub repo: String,
    pub working_dir: PathBuf,
    pub workspace: Option<PtyWorkspaceMetadata>,
    /// Node-side fan-out of PTY output bytes, fed by the devenv link. Every
    /// subscriber (WS attacher) gets a copy. Closes when the shell exits.
    pub output: broadcast::Sender<Vec<u8>>,
    /// Which devenv hosts this session's process.
    pub devenv_ident: String,
    link: Arc<DevenvLink>,
}

impl PtySession {
    /// Current shadow-emulator render, fetched from the devenv. Ordered
    /// after every output frame the link has already forwarded, so an attach
    /// can send it and then stream without a gap.
    pub async fn snapshot(&self) -> Vec<u8> {
        match self.link.snapshot(self.id).await {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(id = %self.id, %err, "devenv snapshot unavailable");
                Vec::new()
            }
        }
    }

    pub async fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()> {
        self.link.resize(self.id, rows, cols).await
    }
}

/// Session records plus node-side handles, with a Postgres pool for
/// persistence. `link` is present on the node runtime and absent on the
/// control plane, which only ever does record-keeping.
pub struct PtyManager {
    pool: Pool,
    link: Option<Arc<DevenvLink>>,
    sessions: RwLock<HashMap<Uuid, Arc<PtySession>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnParams {
    pub id: Option<Uuid>,
    pub node_id: Option<Uuid>,
    pub node_boot_id: Option<Uuid>,
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
            id: None,
            node_id: None,
            node_boot_id: None,
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

mod environment;

use environment::pty_environment;

impl PtyManager {
    /// Record-keeping manager with no devenv: the control plane's shape.
    /// Process operations (spawn, input) fail; metadata and row updates work.
    pub fn new(pool: Pool) -> Arc<Self> {
        Arc::new(Self {
            pool,
            link: None,
            sessions: RwLock::new(HashMap::new()),
        })
    }

    /// Full manager backed by a devenv link. Consumes the link's event stream
    /// to keep rows and handles honest across devenv reconnects and exits.
    pub fn with_devenv(
        pool: Pool,
        link: Arc<DevenvLink>,
        events: mpsc::UnboundedReceiver<LinkEvent>,
    ) -> Arc<Self> {
        let manager = Arc::new(Self {
            pool,
            link: Some(link),
            sessions: RwLock::new(HashMap::new()),
        });
        tokio::spawn(manager.clone().run_events(events));
        manager
    }

    fn link(&self) -> anyhow::Result<&Arc<DevenvLink>> {
        self.link
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("this process hosts no devenv runtime"))
    }

    /// Link connectivity for the node heartbeat: where new sessions route and
    /// whether that devenv is actually dialed in. None on the control plane,
    /// which hosts no link.
    pub async fn devenv_status(&self) -> Option<crate::node_protocol::NodeDevenvStatus> {
        let link = self.link.as_ref()?;
        let current_ident = link.current_ident().await;
        Some(crate::node_protocol::NodeDevenvStatus {
            current_connected: link.is_connected(&current_ident).await,
            connected_idents: link.connected_idents().await,
            current_ident,
        })
    }

    /// Applies devenv link events to the records and handles: adoption on
    /// (re)connect, death on exit, and orphaning for anything the devenv no
    /// longer hosts.
    async fn run_events(self: Arc<Self>, mut events: mpsc::UnboundedReceiver<LinkEvent>) {
        while let Some(event) = events.recv().await {
            match event {
                LinkEvent::Exited { id, exit_code } => {
                    self.mark_dead(id, exit_code).await;
                }
                LinkEvent::Connected { ident, sessions } => {
                    self.sync_with_inventory(&ident, &sessions).await;
                }
            }
        }
    }

    /// On a devenv's (re)connect: adopt hosted sessions this manager has no
    /// handle for, and orphan anything it tracked *on that devenv* that it
    /// no longer hosts — the shell was lost without a supervised exit
    /// (a real exit arrives as `LinkEvent::Exited` and goes to 'dead').
    /// Sessions hosted by other devenvs are not this hello's to judge.
    async fn sync_with_inventory(self: &Arc<Self>, ident: &str, inventory: &[Uuid]) {
        let Ok(link) = self.link() else { return };
        let tracked: Vec<(Uuid, String)> = self
            .sessions
            .read()
            .await
            .values()
            .map(|session| (session.id, session.devenv_ident.clone()))
            .collect();
        for (id, host) in &tracked {
            if host == ident && !inventory.contains(id) {
                self.mark_orphaned(*id).await;
            }
        }
        for id in inventory {
            if tracked.iter().any(|(tracked_id, _)| tracked_id == id) {
                continue;
            }
            let row = match read_meta(&self.pool, *id).await {
                Ok(Some(meta)) => meta,
                Ok(None) => {
                    tracing::warn!(%id, "devenv hosts a session with no record; leaving it alone");
                    continue;
                }
                Err(err) => {
                    tracing::error!(%id, %err, "failed to load session record for adoption");
                    continue;
                }
            };
            let Some(output) = link.output_sender(*id).await else {
                continue;
            };
            let session = Arc::new(PtySession {
                id: *id,
                repo: row.repo.clone(),
                working_dir: row.working_dir.clone(),
                workspace: row.workspace.clone(),
                output,
                devenv_ident: ident.to_string(),
                link: link.clone(),
            });
            self.sessions.write().await.insert(*id, session);
            if let Err(err) = sqlx::query("UPDATE pty_sessions SET devenv_ident = $2 WHERE id = $1")
                .bind(id)
                .bind(ident)
                .execute(&self.pool)
                .await
            {
                tracing::warn!(%id, %err, "failed to record adopted session's devenv");
            }
            tracing::info!(%id, %ident, "adopted devenv-hosted session");
        }
    }
}

/// Session lifecycle and record-keeping.
impl PtyManager {
    /// Count of currently-tracked PTY sessions (live + any still in the
    /// map that haven't been reaped). Drives the app-state stats surface.
    pub async fn live_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    pub async fn live_session_ids(&self) -> Vec<Uuid> {
        let mut ids = self
            .sessions
            .read()
            .await
            .keys()
            .copied()
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    /// Spawn a new PTY + shell: persists the pty_sessions row, then asks the
    /// devenv server to host the process.
    ///
    /// The row goes in before the devenv spawn on purpose: a shell can exit
    /// (and its `Exited` event arrive) faster than this method resumes, and
    /// an exit event that finds no row would leave a live record for a dead
    /// process.
    pub async fn spawn(self: &Arc<Self>, params: SpawnParams) -> anyhow::Result<PtyMetadata> {
        let id = params.id.unwrap_or_else(Uuid::new_v4);
        if self.sessions.read().await.contains_key(&id) {
            anyhow::bail!("PTY session {id} is already live");
        }
        let link = self.link()?.clone();
        let secret_broker_key_path = crate::secret_pty::prepare_pty_credential(id).await?;
        let env = pty_environment(
            id,
            &params.shell,
            secret_broker_key_path.as_ref(),
            params.workspace.as_ref(),
        );

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
                 agent_runtime_agent, agent_runtime_state, agent_runtime_started_at, workspace_id, \
                 node_id, node_boot_id, node_disconnected_at, runtime_end_reason, ended_at, exit_code) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NULL, NULL, NULL, NULL) \
             ON CONFLICT (id) DO UPDATE SET \
                 repo = EXCLUDED.repo, working_dir = EXCLUDED.working_dir, \
                 state = EXCLUDED.state, created_at = EXCLUDED.created_at, \
                 agent_runtime_agent = EXCLUDED.agent_runtime_agent, \
                 agent_runtime_state = EXCLUDED.agent_runtime_state, \
                 agent_runtime_started_at = EXCLUDED.agent_runtime_started_at, \
                 agent_runtime_ended_at = NULL, agent_runtime_exit_code = NULL, \
                 workspace_id = EXCLUDED.workspace_id, node_id = EXCLUDED.node_id, \
                 node_boot_id = EXCLUDED.node_boot_id, node_disconnected_at = NULL, \
                 runtime_end_reason = NULL, ended_at = NULL, exit_code = NULL \
             WHERE pty_sessions.state <> 'live'",
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
        .bind(params.node_id)
        .bind(params.node_boot_id)
        .execute(&self.pool)
        .await?;

        let devenv_ident = match link
            .spawn(HostSpawnSpec {
                id,
                shell: params.shell.clone(),
                args: params.args.clone(),
                working_dir: params.working_dir.clone(),
                env,
                cols: params.cols,
                rows: params.rows,
            })
            .await
        {
            Ok(ident) => ident,
            Err(err) => {
                self.abandon_unspawned_row(id).await;
                return Err(err);
            }
        };
        // Recorded after the spawn because the spawn is what decides the
        // host. An exit event racing this update touches different columns.
        sqlx::query("UPDATE pty_sessions SET devenv_ident = $2 WHERE id = $1")
            .bind(id)
            .bind(&devenv_ident)
            .execute(&self.pool)
            .await
            .ok();

        match link.output_sender(id).await {
            Some(output) => {
                let session = Arc::new(PtySession {
                    id,
                    repo: meta.repo.clone(),
                    working_dir: meta.working_dir.clone(),
                    workspace: meta.workspace.clone(),
                    output,
                    devenv_ident,
                    link,
                });
                self.sessions.write().await.insert(id, session);
            }
            // The shell already exited and the link routed its Exited event;
            // the row is (or is about to be) marked dead. Nothing to track.
            None => {
                tracing::debug!(%id, "session exited before its handle was registered");
            }
        }

        Ok(meta)
    }

    /// Moves one session's shell to the current devenv: the process ends
    /// where it is and a fresh default shell starts on the current toolset,
    /// keeping the session's identity, workspace binding, and working
    /// directory. Neighbouring sessions are untouched.
    pub async fn upgrade(
        self: &Arc<Self>,
        id: Uuid,
        node_id: Option<Uuid>,
        node_boot_id: Option<Uuid>,
    ) -> anyhow::Result<PtyMetadata> {
        let session = self
            .get(id)
            .await
            .ok_or_else(|| anyhow::anyhow!("no live PTY session {id}"))?;
        let link = self.link()?.clone();
        let current = link.current_ident().await;
        if session.devenv_ident == current {
            anyhow::bail!("session is already on the current toolset");
        }
        let repo = session.repo.clone();
        let working_dir = session.working_dir.clone();
        let workspace = session.workspace.clone();
        drop(session);

        link.kill(id).await?;
        // The exit event clears the handle and marks the row dead; the
        // same-id respawn reuses the row (`ON CONFLICT … WHERE state <>
        // 'live'`) and must not race either.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let handle_gone = !self.sessions.read().await.contains_key(&id);
            let row_dead = matches!(
                read_meta(&self.pool, id).await,
                Ok(Some(meta)) if meta.state != PtyState::Live
            );
            if handle_gone && row_dead {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("session {id} did not end in time for its upgrade");
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        self.spawn(SpawnParams {
            id: Some(id),
            node_id,
            node_boot_id,
            repo,
            working_dir,
            workspace,
            shell: default_shell(),
            args: Vec::new(),
            ..Default::default()
        })
        .await
    }

    /// The row was written on the promise of a process; without one it would
    /// sit in the sidebar as a live husk. Deleted also keeps it out of every
    /// listing.
    async fn abandon_unspawned_row(&self, id: Uuid) {
        sqlx::query(
            "UPDATE pty_sessions SET state = 'deleted', ended_at = NOW() \
             WHERE id = $1 AND state = 'live'",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .ok();
        crate::secret_pty::revoke_pty_credential(id).await;
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
        session.link.input(id, bytes).await
    }

    /// Asks the devenv to end the shell (its TERM→grace→KILL ladder), waits
    /// briefly for the death to land, then marks the DB row deleted.
    pub async fn delete(&self, id: Uuid) -> anyhow::Result<()> {
        let session = self.sessions.write().await.remove(&id);
        if let Some(session) = session {
            if session.link.kill(id).await.is_ok() {
                // The exit surfaces through the link and closes the fan-out;
                // bounded wait so a wedged shell cannot wedge the API.
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
                while session.link.output_sender(id).await.is_some()
                    && tokio::time::Instant::now() < deadline
                {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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

    /// Called when a devenv's inventory no longer lists a tracked session:
    /// the process is gone but never reported an exit, so the row stays
    /// resumable rather than reading as a shell that chose to end.
    async fn mark_orphaned(&self, id: Uuid) {
        self.sessions.write().await.remove(&id);
        if let Err(err) = sqlx::query(
            "UPDATE pty_sessions \
             SET state = 'orphaned', \
                 ended_at = NOW(), \
                 runtime_end_reason = 'devenv_inventory_missing', \
                 agent_runtime_state = CASE \
                     WHEN agent_runtime_state IN ('starting', 'running') THEN 'exited' \
                     ELSE agent_runtime_state \
                 END, \
                 agent_runtime_ended_at = CASE \
                     WHEN agent_runtime_state IN ('starting', 'running') THEN NOW() \
                     ELSE agent_runtime_ended_at \
                 END \
             WHERE id = $1 AND state = 'live'",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        {
            tracing::error!(%id, %err, "mark_orphaned failed");
        }
        crate::secret_pty::revoke_pty_credential(id).await;
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
