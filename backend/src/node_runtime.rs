//! Machine-local development runtime.
//!
//! This module is the ownership boundary behind the development-node
//! protocol. It is deliberately usable by both the extracted `sulion-node`
//! process and the transitional in-memory transport: API handlers never need
//! a repo path or PTY handle when node mode is enabled.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use portable_pty::PtySize;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch, Mutex};
use uuid::Uuid;

use crate::agent::AgentType;
use crate::node_protocol::{
    NodeOperationKind, NodeRequestKind, OperationResultPayload, OperationResultStatus,
    RequestResultPayload, TerminalBytesPayload, WireEnvelope,
};
use crate::pty::{PtyManager, PtyMetadata, PtyWorkspaceMetadata, SpawnParams};
use crate::repo_state::RepoStateManager;
use crate::worktree::{DeleteWorkspaceOptions, WorkspaceManager, WorkspaceRecord};

const TERMINAL_ATTACH_BUFFER: usize = 256;
const RAW_FILE_MAX_BYTES: usize = 25 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionLaunch {
    Shell,
    Agent {
        agent: String,
        resume_session_uuid: Option<Uuid>,
    },
    MockTerminal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCreateRequest {
    pub session_id: Uuid,
    pub allocated_workspace_id: Uuid,
    pub existing_workspace_id: Option<Uuid>,
    pub repo: String,
    pub working_dir: Option<String>,
    pub workspace_mode: String,
    pub cols: u16,
    pub rows: u16,
    pub launch: SessionLaunch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    pub session_id: Uuid,
    pub agent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRequest {
    pub id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoCreateRequest {
    pub name: String,
    pub git_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRenameRequest {
    pub old_name: String,
    pub new_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoDeleteRequest {
    pub name: String,
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoPathRequest {
    pub repo: String,
    pub path: Option<String>,
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspacePathRequest {
    pub workspace_id: Uuid,
    pub path: Option<String>,
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageRequest {
    pub path: String,
    pub stage: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStageRequest {
    pub repo: String,
    pub change: StageRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStageRequest {
    pub workspace_id: Uuid,
    pub change: StageRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadRequest {
    pub path: String,
    pub data: String,
}

impl UploadRequest {
    pub fn new(path: String, bytes: &[u8]) -> Self {
        Self {
            path,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    fn into_bytes(self) -> anyhow::Result<Vec<u8>> {
        base64::engine::general_purpose::STANDARD
            .decode(self.data)
            .map_err(|err| anyhow::anyhow!("invalid upload payload: {err}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoUploadRequest {
    pub repo: String,
    pub upload: UploadRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceUploadRequest {
    pub workspace_id: Uuid,
    pub upload: UploadRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawFileResponse {
    pub path: String,
    pub size: usize,
    pub data: String,
}

impl RawFileResponse {
    pub fn into_bytes(self) -> anyhow::Result<Vec<u8>> {
        base64::engine::general_purpose::STANDARD
            .decode(self.data)
            .map_err(|err| anyhow::anyhow!("invalid raw file payload: {err}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInputRequest {
    pub session_id: Uuid,
    pub data: String,
}

impl SessionInputRequest {
    pub fn from_bytes(session_id: Uuid, bytes: &[u8]) -> Self {
        Self {
            session_id,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    fn into_bytes(self) -> anyhow::Result<Vec<u8>> {
        base64::engine::general_purpose::STANDARD
            .decode(self.data)
            .map_err(|err| anyhow::anyhow!("invalid session input: {err}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResizeRequest {
    pub session_id: Uuid,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, thiserror::Error)]
enum RuntimeError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
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
}

impl NodeRuntime {
    pub fn new(
        node_id: Uuid,
        boot_id: Uuid,
        pool: crate::db::Pool,
        repos_root: PathBuf,
        workspaces_root: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            node_id,
            boot_id,
            pty: PtyManager::new(pool.clone()),
            repo_state: RepoStateManager::new(pool.clone(), repos_root.clone()),
            workspace_state: WorkspaceManager::new(
                pool.clone(),
                repos_root.clone(),
                workspaces_root,
            ),
            pool,
            repos_root,
            attachments: Mutex::new(HashMap::new()),
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

    pub async fn execute_operation(
        &self,
        kind: NodeOperationKind,
        request: Value,
    ) -> OperationResultPayload {
        let result = self.execute_operation_inner(kind, request).await;
        operation_result(result)
    }

    async fn execute_operation_inner(
        &self,
        kind: NodeOperationKind,
        request: Value,
    ) -> Result<Value, RuntimeError> {
        match kind {
            NodeOperationKind::ProbeEcho => Ok(json!({ "echo": request })),
            NodeOperationKind::ReconcileInventory => {
                Ok(json!({ "live_session_ids": self.live_session_ids().await }))
            }
            NodeOperationKind::SessionCreate => {
                let request: SessionCreateRequest = decode(request)?;
                serde_json::to_value(self.create_session(request).await?)
                    .map_err(anyhow::Error::from)
                    .map_err(RuntimeError::Internal)
            }
            NodeOperationKind::SessionDelete => {
                let request: ResourceRequest = decode(request)?;
                self.pty.delete(request.id).await?;
                Ok(Value::Null)
            }
            NodeOperationKind::SessionAgentStart => {
                let request: AgentRequest = decode(request)?;
                let agent = parse_agent(&request.agent)?;
                self.pty
                    .mark_agent_starting(request.session_id, agent.as_str())
                    .await?;
                self.pty
                    .send_input(
                        request.session_id,
                        format!("{}\r", agent_launch_command(agent, None, false)).into_bytes(),
                    )
                    .await?;
                Ok(Value::Null)
            }
            NodeOperationKind::SessionAgentInterrupt => {
                let request: ResourceRequest = decode(request)?;
                self.pty.send_input(request.id, b"\x1b".to_vec()).await?;
                Ok(Value::Null)
            }
            NodeOperationKind::RepoCreate => {
                let request: RepoCreateRequest = decode(request)?;
                serde_json::to_value(self.create_repo(request).await?)
                    .map_err(anyhow::Error::from)
                    .map_err(RuntimeError::Internal)
            }
            NodeOperationKind::RepoRename => {
                let request: RepoRenameRequest = decode(request)?;
                crate::api::repo_lifecycle_routes::rename_repo_runtime(
                    &self.pool,
                    &self.repos_root,
                    &request.old_name,
                    &request.new_name,
                )
                .await
                .map_err(runtime_api_error)?;
                Ok(json!({
                    "name": request.new_name,
                    "path": self.repos_root.join(&request.new_name),
                }))
            }
            NodeOperationKind::RepoDelete => {
                let request: RepoDeleteRequest = decode(request)?;
                crate::api::repo_lifecycle_routes::delete_repo_runtime(
                    &self.pool,
                    &self.repos_root,
                    &request.name,
                    request.force,
                )
                .await
                .map_err(runtime_api_error)?;
                Ok(Value::Null)
            }
            NodeOperationKind::WorkspaceDelete => {
                let request: (ResourceRequest, DeleteWorkspaceOptions) = decode(request)?;
                self.workspace_state
                    .delete_workspace(request.0.id, request.1)
                    .await?;
                Ok(Value::Null)
            }
            NodeOperationKind::RepoRefresh
            | NodeOperationKind::RepoStage
            | NodeOperationKind::RepoUpload
            | NodeOperationKind::WorkspaceRefresh
            | NodeOperationKind::WorkspaceStage
            | NodeOperationKind::WorkspaceUpload => Err(RuntimeError::BadRequest(
                "non-lifecycle mutation must use a node request".into(),
            )),
        }
    }

    pub async fn execute_request(
        &self,
        kind: NodeRequestKind,
        request: Value,
    ) -> RequestResultPayload {
        request_result(self.execute_request_inner(kind, request).await)
    }

    async fn execute_request_inner(
        &self,
        kind: NodeRequestKind,
        request: Value,
    ) -> Result<Value, RuntimeError> {
        match kind {
            NodeRequestKind::SessionInput => {
                let request: SessionInputRequest = decode(request)?;
                self.pty
                    .send_input(request.session_id, request.into_bytes()?)
                    .await?;
                Ok(Value::Null)
            }
            NodeRequestKind::SessionResize => {
                let request: SessionResizeRequest = decode(request)?;
                let session = self
                    .pty
                    .get(request.session_id)
                    .await
                    .ok_or_else(|| RuntimeError::NotFound("session is not live".into()))?;
                session
                    .resize
                    .send(PtySize {
                        rows: request.rows,
                        cols: request.cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .await
                    .map_err(|_| RuntimeError::NotFound("session is not live".into()))?;
                Ok(Value::Null)
            }
            NodeRequestKind::RepoRefresh => {
                let request: RepoPathRequest = decode(request)?;
                self.repo_root(&request.repo)?;
                self.repo_state.request_refresh(&request.repo).await?;
                Ok(Value::Null)
            }
            NodeRequestKind::RepoStage => {
                let request: RepoStageRequest = decode(request)?;
                let root = self.repo_root(&request.repo)?;
                crate::git::stage_path(root, request.change.path, request.change.stage).await?;
                self.repo_state.request_refresh(&request.repo).await?;
                Ok(Value::Null)
            }
            NodeRequestKind::RepoUpload => {
                let request: RepoUploadRequest = decode(request)?;
                let root = self.repo_root(&request.repo)?;
                let path = request.upload.path.clone();
                let bytes = request.upload.into_bytes()?;
                let written = crate::workspace::write_file(root, path, bytes.clone()).await?;
                self.repo_state.request_refresh(&request.repo).await?;
                Ok(json!({"path": written, "size": bytes.len()}))
            }
            NodeRequestKind::RepoFiles => {
                let request: RepoPathRequest = decode(request)?;
                let root = self.repo_root(&request.repo)?;
                let dirty = crate::repo_state::load_dirty_paths(&self.pool, &request.repo)
                    .await
                    .unwrap_or_else(|_| empty_repo_dirty(&request.repo));
                value(
                    crate::workspace::list_dir(
                        root,
                        request.path.unwrap_or_default(),
                        !request.all,
                        dirty.dirty_by_path,
                        dirty.diff_stats_by_path,
                    )
                    .await?,
                )
            }
            NodeRequestKind::RepoFilePreview => {
                let request: RepoPathRequest = decode(request)?;
                let path = required_path(request.path)?;
                value(
                    crate::api::file_content::build_preview(self.repo_root(&request.repo)?, &path)
                        .await
                        .map_err(|err| RuntimeError::BadRequest(err.to_string()))?,
                )
            }
            NodeRequestKind::RepoFileRaw => {
                let request: RepoPathRequest = decode(request)?;
                self.raw_file(self.repo_root(&request.repo)?, required_path(request.path)?)
                    .await
            }
            NodeRequestKind::RepoDiff => {
                let request: RepoPathRequest = decode(request)?;
                Ok(json!({
                    "diff": crate::git::read_diff(self.repo_root(&request.repo)?, request.path).await?
                }))
            }
            NodeRequestKind::RepoDirtyPaths => {
                let request: RepoPathRequest = decode(request)?;
                self.repo_root(&request.repo)?;
                value(crate::repo_state::load_dirty_paths(&self.pool, &request.repo).await?)
            }
            NodeRequestKind::WorkspaceRefresh => {
                let request: WorkspacePathRequest = decode(request)?;
                self.workspace_state
                    .load_workspace(request.workspace_id)
                    .await
                    .map_err(|_| RuntimeError::NotFound("workspace not found".into()))?;
                self.workspace_state
                    .request_refresh(request.workspace_id)
                    .await?;
                Ok(Value::Null)
            }
            NodeRequestKind::WorkspaceStage => {
                let request: WorkspaceStageRequest = decode(request)?;
                let workspace = self.load_workspace(request.workspace_id).await?;
                crate::git::stage_path(workspace.path, request.change.path, request.change.stage)
                    .await?;
                self.workspace_state
                    .request_refresh(request.workspace_id)
                    .await?;
                Ok(Value::Null)
            }
            NodeRequestKind::WorkspaceUpload => {
                let request: WorkspaceUploadRequest = decode(request)?;
                let workspace = self.load_workspace(request.workspace_id).await?;
                let path = request.upload.path.clone();
                let bytes = request.upload.into_bytes()?;
                let written =
                    crate::workspace::write_file(workspace.path, path, bytes.clone()).await?;
                self.workspace_state
                    .request_refresh(request.workspace_id)
                    .await?;
                Ok(json!({"path": written, "size": bytes.len()}))
            }
            NodeRequestKind::WorkspaceFiles => {
                let request: WorkspacePathRequest = decode(request)?;
                let workspace = self.load_workspace(request.workspace_id).await?;
                let dirty =
                    crate::worktree::load_workspace_dirty_paths(&self.pool, request.workspace_id)
                        .await
                        .unwrap_or_else(|_| empty_workspace_dirty(request.workspace_id));
                value(
                    crate::workspace::list_dir(
                        workspace.path,
                        request.path.unwrap_or_default(),
                        !request.all,
                        dirty.dirty_by_path,
                        dirty.diff_stats_by_path,
                    )
                    .await?,
                )
            }
            NodeRequestKind::WorkspaceFilePreview => {
                let request: WorkspacePathRequest = decode(request)?;
                let workspace = self.load_workspace(request.workspace_id).await?;
                let path = required_path(request.path)?;
                value(
                    crate::api::file_content::build_preview(workspace.path, &path)
                        .await
                        .map_err(|err| RuntimeError::BadRequest(err.to_string()))?,
                )
            }
            NodeRequestKind::WorkspaceFileRaw => {
                let request: WorkspacePathRequest = decode(request)?;
                let workspace = self.load_workspace(request.workspace_id).await?;
                self.raw_file(workspace.path, required_path(request.path)?)
                    .await
            }
            NodeRequestKind::WorkspaceDiff => {
                let request: WorkspacePathRequest = decode(request)?;
                let workspace = self.load_workspace(request.workspace_id).await?;
                Ok(json!({
                    "diff": crate::git::read_diff(workspace.path, request.path).await?
                }))
            }
            NodeRequestKind::WorkspaceDirtyPaths => {
                let request: WorkspacePathRequest = decode(request)?;
                value(
                    crate::worktree::load_workspace_dirty_paths(&self.pool, request.workspace_id)
                        .await?,
                )
            }
        }
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
            let snapshot = session.emulator.snapshot();
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

    async fn claim_discovered_resources(&self) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO repos (name, path, node_id) \
             SELECT repo_name, path, $1 FROM repo_runtime_state WHERE exists = TRUE \
             ON CONFLICT (name) DO UPDATE SET path = EXCLUDED.path, node_id = EXCLUDED.node_id",
        )
        .bind(self.node_id)
        .execute(&self.pool)
        .await?;
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
                "isolated" | "worktree" => {
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
                cols: request.cols.clamp(20, 500),
                rows: request.rows.clamp(5, 300),
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

    async fn create_repo(&self, request: RepoCreateRequest) -> Result<Value, RuntimeError> {
        validate_repo_name(&request.name)?;
        tokio::fs::create_dir_all(&self.repos_root).await?;
        let destination = self.repos_root.join(&request.name);
        if destination.exists() {
            return Err(RuntimeError::BadRequest("repo already exists".into()));
        }
        let output = if let Some(url) = request.git_url {
            tokio::process::Command::new("git")
                .arg("clone")
                .arg(url)
                .arg(&destination)
                .output()
                .await?
        } else {
            tokio::fs::create_dir_all(&destination).await?;
            tokio::process::Command::new("git")
                .arg("init")
                .arg(&destination)
                .output()
                .await?
        };
        if !output.status.success() {
            return Err(RuntimeError::BadRequest(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        self.repo_state
            .upsert_repo(&request.name, &destination)
            .await?;
        sqlx::query(
            "INSERT INTO repos (name, path, node_id) VALUES ($1, $2, $3) \
             ON CONFLICT (name) DO UPDATE SET path = EXCLUDED.path, node_id = EXCLUDED.node_id",
        )
        .bind(&request.name)
        .bind(destination.to_string_lossy().as_ref())
        .bind(self.node_id)
        .execute(&self.pool)
        .await
        .map_err(anyhow::Error::from)?;
        Ok(json!({"name": request.name, "path": destination}))
    }

    fn repo_root(&self, name: &str) -> Result<PathBuf, RuntimeError> {
        validate_repo_name(name)?;
        let root = self.repos_root.join(name);
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(format!("repo not found: {name}")));
        }
        Ok(root)
    }

    async fn load_workspace(&self, id: Uuid) -> Result<WorkspaceRecord, RuntimeError> {
        self.workspace_state
            .load_workspace(id)
            .await
            .map_err(|_| RuntimeError::NotFound(format!("workspace not found: {id}")))
    }

    async fn raw_file(&self, root: PathBuf, path: String) -> Result<Value, RuntimeError> {
        let (_, bytes) = crate::workspace::read_file(root, path.clone()).await?;
        if bytes.len() > RAW_FILE_MAX_BYTES {
            return Err(RuntimeError::BadRequest(format!(
                "file exceeds {RAW_FILE_MAX_BYTES} bytes"
            )));
        }
        value(RawFileResponse {
            path,
            size: bytes.len(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        })
    }
}

fn operation_result(result: Result<Value, RuntimeError>) -> OperationResultPayload {
    match result {
        Ok(result) => OperationResultPayload {
            status: OperationResultStatus::Succeeded,
            result: Some(result),
            error_code: None,
            error_message: None,
        },
        Err(err) => OperationResultPayload {
            status: OperationResultStatus::Failed,
            result: None,
            error_code: Some(err.code().into()),
            error_message: Some(err.to_string()),
        },
    }
}

fn request_result(result: Result<Value, RuntimeError>) -> RequestResultPayload {
    match result {
        Ok(result) => RequestResultPayload {
            status: OperationResultStatus::Succeeded,
            result: Some(result),
            error_code: None,
            error_message: None,
        },
        Err(err) => RequestResultPayload {
            status: OperationResultStatus::Failed,
            result: None,
            error_code: Some(err.code().into()),
            error_message: Some(err.to_string()),
        },
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

fn runtime_api_error(error: crate::api::ApiError) -> RuntimeError {
    match error {
        crate::api::ApiError::NotFound => RuntimeError::NotFound("not found".into()),
        crate::api::ApiError::BadRequest(message) => RuntimeError::BadRequest(message),
        other => RuntimeError::Internal(anyhow::anyhow!(other)),
    }
}

fn validate_repo_name(name: &str) -> Result<(), RuntimeError> {
    if name.is_empty()
        || name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
    {
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

pub fn terminal_attach_channel() -> (mpsc::Sender<WireEnvelope>, mpsc::Receiver<WireEnvelope>) {
    mpsc::channel(TERMINAL_ATTACH_BUFFER)
}
