//! Machine-local development runtime.
//!
//! This module is the ownership boundary behind the development-node
//! protocol. It is deliberately usable by both the extracted `sulion-node`
//! process and the transitional in-memory transport: API handlers never need
//! a repo path or PTY handle when node mode is enabled.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, watch, Mutex};
use uuid::Uuid;

use crate::agent::AgentType;
use crate::node_protocol::{
    NodeRequestKind, RequestResultPayload, RequestResultStatus, TerminalBytesPayload, WireEnvelope,
};
use crate::pty::{PtyManager, PtyMetadata, PtyWorkspaceMetadata, SpawnParams};
use crate::repo_state::RepoStateManager;
use crate::worktree::{WorkspaceManager, WorkspaceRecord};

mod host;
mod messages;
mod requests;

pub use host::HostProbe;
pub use messages::*;

#[derive(Debug, thiserror::Error)]
enum RuntimeError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    // Not `#[error(transparent)]`: that renders only the outermost context and
    // drops the chain, so an operator sees the label of a failure rather than
    // its cause. The whole chain goes to the browser because the alternative
    // is asking someone to go read a node's logs to learn what broke.
    #[error("{}", render_chain(.0))]
    Internal(#[from] anyhow::Error),
}

/// Renders an error and everything beneath it as `outer: inner: root`.
fn render_chain(error: &anyhow::Error) -> String {
    let mut rendered = error.to_string();
    for cause in error.chain().skip(1) {
        rendered.push_str(": ");
        rendered.push_str(&cause.to_string());
    }
    rendered
}

impl From<std::io::Error> for RuntimeError {
    fn from(value: std::io::Error) -> Self {
        Self::Internal(value.into())
    }
}

impl From<sqlx::Error> for RuntimeError {
    fn from(value: sqlx::Error) -> Self {
        Self::Internal(value.into())
    }
}

impl From<crate::repo_lifecycle::RepoLifecycleError> for RuntimeError {
    fn from(value: crate::repo_lifecycle::RepoLifecycleError) -> Self {
        use crate::repo_lifecycle::RepoLifecycleError as Error;
        match value {
            Error::NotFound => Self::NotFound("repo not found".into()),
            Error::BadRequest(message) => Self::BadRequest(message),
            Error::Internal(err) => Self::Internal(err),
            Error::Db(err) => Self::Internal(err.into()),
            Error::Io(err) => Self::Internal(err.into()),
        }
    }
}

impl From<crate::file_preview::PreviewError> for RuntimeError {
    fn from(value: crate::file_preview::PreviewError) -> Self {
        use crate::file_preview::PreviewError;
        match value {
            PreviewError::NotFound => Self::NotFound("no such file".into()),
            PreviewError::BadPath(message) => Self::BadRequest(message),
            PreviewError::Io(err) => Self::Internal(err.into()),
        }
    }
}

impl RuntimeError {
    fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::NotFound(_) => "not_found",
            Self::Internal(_) => "runtime_error",
        }
    }
}

pub struct NodeRuntime {
    node_id: Uuid,
    boot_id: Uuid,
    pool: crate::db::Pool,
    repos_root: PathBuf,
    pty: Arc<PtyManager>,
    repo_state: Arc<RepoStateManager>,
    workspace_state: Arc<WorkspaceManager>,
    attachments: Mutex<HashMap<Uuid, watch::Sender<bool>>>,
    host: HostProbe,
}

impl NodeRuntime {
    pub fn new(
        node_id: Uuid,
        boot_id: Uuid,
        pool: crate::db::Pool,
        repos_root: PathBuf,
        workspaces_root: PathBuf,
        devenv_link: Arc<crate::devenv::link::DevenvLink>,
        devenv_events: mpsc::UnboundedReceiver<crate::devenv::link::LinkEvent>,
    ) -> Arc<Self> {
        Arc::new(Self {
            node_id,
            boot_id,
            pty: PtyManager::with_devenv(pool.clone(), devenv_link, devenv_events),
            repo_state: RepoStateManager::new(pool.clone(), repos_root.clone()),
            workspace_state: WorkspaceManager::new(
                pool.clone(),
                repos_root.clone(),
                workspaces_root,
            ),
            pool,
            repos_root,
            attachments: Mutex::new(HashMap::new()),
            host: HostProbe::new(),
        })
    }

    pub fn node_id(&self) -> Uuid {
        self.node_id
    }

    pub fn boot_id(&self) -> Uuid {
        self.boot_id
    }

    pub fn pty(&self) -> Arc<PtyManager> {
        self.pty.clone()
    }

    pub fn repo_state(&self) -> Arc<RepoStateManager> {
        self.repo_state.clone()
    }

    pub fn workspace_state(&self) -> Arc<WorkspaceManager> {
        self.workspace_state.clone()
    }

    pub async fn live_session_ids(&self) -> Vec<Uuid> {
        self.pty.live_session_ids().await
    }

    pub async fn host_stats(&self) -> crate::node_protocol::NodeHostStats {
        self.host.sample().await
    }

    pub async fn open_terminal(
        self: &Arc<Self>,
        stream_id: Uuid,
        session_id: Uuid,
        outbound: mpsc::Sender<WireEnvelope>,
    ) -> anyhow::Result<()> {
        let session = self
            .pty
            .get(session_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("session is not live"))?;
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        if let Some(previous) = self.attachments.lock().await.insert(stream_id, cancel_tx) {
            let _ = previous.send(true);
        }
        let runtime = self.clone();
        tokio::spawn(async move {
            let stream = TerminalStream {
                outbound,
                node_id: runtime.node_id,
                boot_id: runtime.boot_id,
                session_id,
                stream_id,
            };
            let mut sequence = 0_u64;
            let snapshot = session.snapshot().await;
            if !snapshot.is_empty()
                && stream
                    .send_bytes(sequence, "terminal.snapshot", &snapshot)
                    .await
                    .is_err()
            {
                runtime.attachments.lock().await.remove(&stream_id);
                return;
            }
            sequence += 1;
            let mut ready = WireEnvelope::new(runtime.node_id, runtime.boot_id, "terminal.ready");
            ready.session_id = Some(session_id);
            ready.stream_id = Some(stream_id);
            ready.sequence = Some(sequence);
            if stream.outbound.send(ready).await.is_err() {
                runtime.attachments.lock().await.remove(&stream_id);
                return;
            }
            sequence += 1;
            let mut output = session.output.subscribe();
            loop {
                tokio::select! {
                    changed = cancel_rx.changed() => {
                        if changed.is_err() || *cancel_rx.borrow() {
                            break;
                        }
                    }
                    bytes = output.recv() => {
                        match bytes {
                            Ok(bytes) => {
                                if stream.send_bytes(sequence, "terminal.output", &bytes).await.is_err() {
                                    break;
                                }
                                sequence += 1;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                                tracing::warn!(%session_id, %stream_id, count, "node terminal attachment lagged");
                                break;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                let mut dead =
                                    WireEnvelope::new(runtime.node_id, runtime.boot_id, "terminal.dead");
                                dead.session_id = Some(session_id);
                                dead.stream_id = Some(stream_id);
                                dead.sequence = Some(sequence);
                                let _ = stream.outbound.send(dead).await;
                                break;
                            }
                        }
                    }
                }
            }
            runtime.attachments.lock().await.remove(&stream_id);
        });
        Ok(())
    }

    pub async fn close_terminal(&self, stream_id: Uuid) {
        if let Some(cancel) = self.attachments.lock().await.remove(&stream_id) {
            let _ = cancel.send(true);
        }
    }

    pub async fn run_background_managers(self: Arc<Self>) {
        if let Err(err) = self.repo_state.sync_repos_once().await {
            tracing::warn!(%err, "initial node repo state sync failed");
        }
        if let Err(err) = self.workspace_state.sync_main_workspaces_once().await {
            tracing::warn!(%err, "initial node workspace sync failed");
        }
        if let Err(err) = self.claim_discovered_resources().await {
            tracing::warn!(%err, "failed to assign discovered resources to node");
        }
        tokio::spawn(self.repo_state.clone().run());
        tokio::spawn(self.workspace_state.clone().run());
    }

    /// Claims workspaces this node is responsible for.
    ///
    /// Repos are no longer claimed: ownership lived in `repos.node_id`, which
    /// only this node could ever be, and claiming at startup was what made a
    /// repo discovered later unreachable. `repo_runtime_state` is the registry;
    /// the node answers "does this repo exist" from disk on each request.
    async fn claim_discovered_resources(&self) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE workspaces SET node_id = $1, updated_at = NOW() \
             WHERE state <> 'deleted' AND (node_id IS NULL OR node_id = $1)",
        )
        .bind(self.node_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn create_session(
        &self,
        request: SessionCreateRequest,
    ) -> Result<PtyMetadata, RuntimeError> {
        let repo_root = self.repo_root(&request.repo)?;
        let workspace = if let Some(id) = request.existing_workspace_id {
            let workspace = self.load_workspace(id).await?;
            if workspace.repo_name != request.repo || workspace.state != "active" {
                return Err(RuntimeError::BadRequest(
                    "workspace does not belong to the requested repo or is inactive".into(),
                ));
            }
            sqlx::query(
                "UPDATE workspaces SET node_id = COALESCE(node_id, $2), updated_at = NOW() \
                 WHERE id = $1",
            )
            .bind(id)
            .bind(self.node_id)
            .execute(&self.pool)
            .await
            .map_err(anyhow::Error::from)?;
            workspace
        } else {
            match request.workspace_mode.as_str() {
                "main" => {
                    self.workspace_state
                        .ensure_main_workspace_owned(
                            &request.repo,
                            &repo_root,
                            Some(request.allocated_workspace_id),
                            Some(self.node_id),
                        )
                        .await?
                }
                "isolated" => {
                    self.workspace_state
                        .create_worktree_workspace_owned(
                            &request.repo,
                            request.allocated_workspace_id,
                            Some(self.node_id),
                        )
                        .await?
                }
                _ => {
                    return Err(RuntimeError::BadRequest(
                        "workspace_mode must be main or isolated".into(),
                    ))
                }
            }
        };
        let working_dir = match request.working_dir {
            Some(relative) if request.workspace_mode == "main" => {
                let (path, _) = crate::workspace::resolve_in_repo(&workspace.path, &relative)
                    .map_err(|err| RuntimeError::BadRequest(err.to_string()))?;
                if !path.is_dir() {
                    return Err(RuntimeError::BadRequest(
                        "working directory does not exist".into(),
                    ));
                }
                path
            }
            Some(_) => {
                return Err(RuntimeError::BadRequest(
                    "working_dir is supported only for the main workspace".into(),
                ))
            }
            None => workspace.path.clone(),
        };
        let (shell, args, initial_agent) = self.resolve_launch(&request.launch)?;
        let metadata = self
            .pty
            .spawn(SpawnParams {
                id: Some(request.session_id),
                node_id: Some(self.node_id),
                node_boot_id: Some(self.boot_id),
                repo: request.repo,
                working_dir,
                workspace: Some(pty_workspace_metadata(&workspace)),
                shell,
                args,
                cols: request.cols,
                rows: request.rows,
                initial_agent_runtime_agent: initial_agent,
            })
            .await?;
        self.workspace_state
            .bind_created_session(workspace.id, metadata.id)
            .await?;
        Ok(metadata)
    }

    fn resolve_launch(
        &self,
        launch: &SessionLaunch,
    ) -> Result<(PathBuf, Vec<String>, Option<String>), RuntimeError> {
        match launch {
            SessionLaunch::Shell => Ok((crate::pty::default_shell(), Vec::new(), None)),
            SessionLaunch::MockTerminal => {
                if !crate::e2e::fixtures_enabled() {
                    return Err(RuntimeError::BadRequest(
                        "e2e fixtures are disabled on the node".into(),
                    ));
                }
                let path = crate::e2e::mock_terminal_script_path(&self.repos_root);
                if !path.is_file() {
                    return Err(RuntimeError::BadRequest(
                        "mock terminal fixture is missing".into(),
                    ));
                }
                Ok((path, Vec::new(), None))
            }
            SessionLaunch::Agent {
                agent,
                resume_session_uuid,
            } => {
                let agent = parse_agent(agent)?;
                let command = agent_launch_command(agent, *resume_session_uuid, true);
                Ok((
                    PathBuf::from("/bin/bash"),
                    vec!["-c".into(), command],
                    Some(agent.as_str().into()),
                ))
            }
        }
    }
}

fn request_result(
    kind: NodeRequestKind,
    result: Result<Value, RuntimeError>,
) -> RequestResultPayload {
    match result {
        Ok(result) => RequestResultPayload {
            status: RequestResultStatus::Succeeded,
            result: Some(result),
            error_code: None,
            error_message: None,
        },
        Err(err) => {
            // The failure is on its way to a browser, but the node is where it
            // happened and the only place with the surrounding context. Without
            // this, every node-side failure is invisible in the node's own log
            // and the only symptom is an error string in the UI.
            match &err {
                RuntimeError::Internal(cause) => {
                    tracing::error!(request = ?kind, error = ?cause, "node request failed")
                }
                other => tracing::warn!(request = ?kind, error = %other, "node request rejected"),
            }
            RequestResultPayload {
                status: RequestResultStatus::Failed,
                result: None,
                error_code: Some(err.code().into()),
                error_message: Some(err.to_string()),
            }
        }
    }
}

fn decode<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, RuntimeError> {
    serde_json::from_value(value)
        .map_err(|err| RuntimeError::BadRequest(format!("invalid request payload: {err}")))
}

fn value<T: Serialize>(value: T) -> Result<Value, RuntimeError> {
    serde_json::to_value(value)
        .map_err(anyhow::Error::from)
        .map_err(RuntimeError::Internal)
}

fn required_path(path: Option<String>) -> Result<String, RuntimeError> {
    path.filter(|path| !path.trim().is_empty())
        .ok_or_else(|| RuntimeError::BadRequest("path is required".into()))
}

fn validate_repo_name(name: &str) -> Result<(), RuntimeError> {
    if !crate::workspace::is_valid_repo_name(name) {
        return Err(RuntimeError::BadRequest("invalid repo name".into()));
    }
    Ok(())
}

fn parse_agent(value: &str) -> Result<AgentType, RuntimeError> {
    AgentType::parse(value)
        .map_err(|_| RuntimeError::BadRequest("agent must be claude or codex".into()))
}

fn agent_launch_command(
    agent: AgentType,
    resume_session_uuid: Option<Uuid>,
    append_exec_bash: bool,
) -> String {
    let mut agent_args = match agent {
        AgentType::Claude => vec!["--dangerously-skip-permissions".to_string()],
        AgentType::Codex => vec!["--yolo".to_string()],
    };
    if let Some(session_id) = resume_session_uuid {
        match agent {
            AgentType::Claude => {
                agent_args.push("--resume".into());
                agent_args.push(session_id.to_string());
            }
            AgentType::Codex => {
                agent_args.push("resume".into());
                agent_args.push(session_id.to_string());
            }
        }
    }
    let mut parts = vec![
        shell_quote(&crate::agent::binary_path()),
        "agent-launcher".into(),
        "--type".into(),
        agent.as_str().into(),
        "--mode".into(),
        "real".into(),
        "--".into(),
    ];
    parts.extend(agent_args.iter().map(|arg| shell_quote_str(arg)));
    let mut command = parts.join(" ");
    if append_exec_bash {
        command.push_str(" ; exec bash");
    }
    command
}

fn shell_quote(path: &Path) -> String {
    shell_quote_str(&path.to_string_lossy())
}

fn shell_quote_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn pty_workspace_metadata(record: &WorkspaceRecord) -> PtyWorkspaceMetadata {
    PtyWorkspaceMetadata {
        id: record.id,
        repo_name: record.repo_name.clone(),
        kind: record.kind.clone(),
        path: record.path.clone(),
        branch_name: record.branch_name.clone(),
        base_ref: record.base_ref.clone(),
        base_sha: record.base_sha.clone(),
        merge_target: record.merge_target.clone(),
    }
}

fn empty_repo_dirty(repo: &str) -> crate::repo_state::RepoDirtyPaths {
    crate::repo_state::RepoDirtyPaths {
        repo: repo.into(),
        git_revision: 0,
        dirty_by_path: HashMap::new(),
        diff_stats_by_path: HashMap::new(),
    }
}

fn empty_workspace_dirty(workspace_id: Uuid) -> crate::worktree::WorkspaceDirtyPaths {
    crate::worktree::WorkspaceDirtyPaths {
        workspace_id,
        git_revision: 0,
        dirty_by_path: HashMap::new(),
        diff_stats_by_path: HashMap::new(),
    }
}

struct TerminalStream {
    outbound: mpsc::Sender<WireEnvelope>,
    node_id: Uuid,
    boot_id: Uuid,
    session_id: Uuid,
    stream_id: Uuid,
}

impl TerminalStream {
    async fn send_bytes(&self, sequence: u64, kind: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let mut envelope = WireEnvelope::new(self.node_id, self.boot_id, kind);
        envelope.session_id = Some(self.session_id);
        envelope.stream_id = Some(self.stream_id);
        envelope.sequence = Some(sequence);
        envelope.payload = serde_json::to_value(TerminalBytesPayload::from_bytes(bytes))?;
        self.outbound
            .send(envelope)
            .await
            .map_err(|_| anyhow::anyhow!("node connection closed"))
    }
}
