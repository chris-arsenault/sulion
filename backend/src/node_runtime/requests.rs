//! Machine-local development runtime.
//!
//! This module is the ownership boundary behind the development-node
//! protocol. It is deliberately usable by both the extracted `sulion-node`
//! process and the transitional in-memory transport: API handlers never need
//! a repo path or PTY handle when node mode is enabled.

use std::path::PathBuf;

use base64::Engine;
use portable_pty::PtySize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::node_protocol::{NodeRequestKind, RequestResultPayload};
use crate::worktree::{DeleteWorkspaceOptions, WorkspaceRecord};

const RAW_FILE_MAX_BYTES: usize = 25 * 1024 * 1024;

use super::*;

impl NodeRuntime {
    pub async fn execute_request(
        &self,
        kind: NodeRequestKind,
        request: Value,
    ) -> RequestResultPayload {
        request_result(kind, self.execute_request_inner(kind, request).await)
    }

    async fn execute_request_inner(
        &self,
        kind: NodeRequestKind,
        request: Value,
    ) -> Result<Value, RuntimeError> {
        match kind {
            NodeRequestKind::ProbeEcho => Ok(json!({ "echo": request })),
            NodeRequestKind::SessionAgentInterrupt
            | NodeRequestKind::SessionAgentStart
            | NodeRequestKind::SessionCreate
            | NodeRequestKind::SessionDelete
            | NodeRequestKind::SessionInput
            | NodeRequestKind::SessionResize => self.execute_session_request(kind, request).await,
            NodeRequestKind::RepoCreate
            | NodeRequestKind::RepoDelete
            | NodeRequestKind::RepoDiff
            | NodeRequestKind::RepoDirtyPaths
            | NodeRequestKind::RepoFilePreview
            | NodeRequestKind::RepoFileRaw
            | NodeRequestKind::RepoFiles
            | NodeRequestKind::RepoRefresh
            | NodeRequestKind::RepoRename
            | NodeRequestKind::RepoStage
            | NodeRequestKind::RepoUpload => self.execute_repo_request(kind, request).await,
            NodeRequestKind::WorkspaceDelete
            | NodeRequestKind::WorkspaceDiff
            | NodeRequestKind::WorkspaceDirtyPaths
            | NodeRequestKind::WorkspaceFilePreview
            | NodeRequestKind::WorkspaceFileRaw
            | NodeRequestKind::WorkspaceFiles
            | NodeRequestKind::WorkspaceRefresh
            | NodeRequestKind::WorkspaceStage
            | NodeRequestKind::WorkspaceUpload => {
                self.execute_workspace_request(kind, request).await
            }
        }
    }

    /// PTY lifecycle, agent control, input, and resize.
    async fn execute_session_request(
        &self,
        kind: NodeRequestKind,
        request: Value,
    ) -> Result<Value, RuntimeError> {
        match kind {
            NodeRequestKind::SessionCreate => {
                let request: SessionCreateRequest = decode(request)?;
                value(self.create_session(request).await?)
            }
            NodeRequestKind::SessionDelete => {
                let request: ResourceRequest = decode(request)?;
                self.pty.delete(request.id).await?;
                Ok(Value::Null)
            }
            NodeRequestKind::SessionAgentStart => {
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
            NodeRequestKind::SessionAgentInterrupt => {
                let request: ResourceRequest = decode(request)?;
                self.pty.send_input(request.id, b"\x1b".to_vec()).await?;
                Ok(Value::Null)
            }
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
            // Unreachable through `execute_request_inner`, which routes by
            // the same grouping. A refusal rather than a panic, so a kind
            // added to only one of the two places cannot crash a node.
            _ => Err(RuntimeError::BadRequest(format!(
                "request {kind:?} is not handled here"
            ))),
        }
    }

    /// Repository creation, rename, delete, and file access.
    async fn execute_repo_request(
        &self,
        kind: NodeRequestKind,
        request: Value,
    ) -> Result<Value, RuntimeError> {
        match kind {
            NodeRequestKind::RepoCreate => {
                let request: RepoCreateRequest = decode(request)?;
                value(self.create_repo(request).await?)
            }
            NodeRequestKind::RepoRename => {
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
            NodeRequestKind::RepoDelete => {
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
            // Unreachable through `execute_request_inner`, which routes by
            // the same grouping. A refusal rather than a panic, so a kind
            // added to only one of the two places cannot crash a node.
            _ => Err(RuntimeError::BadRequest(format!(
                "request {kind:?} is not handled here"
            ))),
        }
    }

    /// Worktree lifecycle and file access.
    async fn execute_workspace_request(
        &self,
        kind: NodeRequestKind,
        request: Value,
    ) -> Result<Value, RuntimeError> {
        match kind {
            NodeRequestKind::WorkspaceDelete => {
                let request: (ResourceRequest, DeleteWorkspaceOptions) = decode(request)?;
                self.workspace_state
                    .delete_workspace(request.0.id, request.1)
                    .await?;
                Ok(Value::Null)
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
            // Unreachable through `execute_request_inner`, which routes by
            // the same grouping. A refusal rather than a panic, so a kind
            // added to only one of the two places cannot crash a node.
            _ => Err(RuntimeError::BadRequest(format!(
                "request {kind:?} is not handled here"
            ))),
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
}

/// Path and record resolution shared by the request handlers above.
impl NodeRuntime {
    pub(super) fn repo_root(&self, name: &str) -> Result<PathBuf, RuntimeError> {
        validate_repo_name(name)?;
        let root = self.repos_root.join(name);
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(format!("repo not found: {name}")));
        }
        Ok(root)
    }

    pub(super) async fn load_workspace(&self, id: Uuid) -> Result<WorkspaceRecord, RuntimeError> {
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
