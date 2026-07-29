//! Workspace-scoped filesystem and git handlers. Repo-scoped routes
//! remain canonical checkout operations; these routes target a specific
//! Sulion workspace/worktree.

use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::file_content::{self, FileResponse};
use super::node_proxy;
use super::repo_routes::{file_trace_response, read_uploads, FileTraceResponse};
use super::routes::{ApiError, ApiResult};
use crate::node_protocol::NodeRequestKind;
use crate::node_runtime::{
    RawFileResponse, ResourceRequest, StageRequest, UploadRequest, WorkspacePathRequest,
    WorkspaceStageRequest, WorkspaceUploadRequest,
};
use crate::{ingest, workspace as fs_workspace, worktree, AppState};

#[derive(Deserialize)]
pub(super) struct FilesQuery {
    path: Option<String>,
    all: Option<bool>,
}

#[derive(Deserialize)]
pub(super) struct FileQuery {
    path: String,
}

#[derive(Deserialize)]
pub(super) struct DiffQuery {
    path: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct DiffResponse {
    diff: String,
}

#[derive(Deserialize)]
pub(super) struct StageReq {
    path: String,
    stage: bool,
}

#[derive(Serialize, Deserialize)]
pub(super) struct UploadResponse {
    path: String,
    size: u64,
}

#[derive(Deserialize)]
pub(super) struct UploadQuery {
    path: Option<String>,
}

pub(super) async fn list_workspaces(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<worktree::WorkspaceView>>> {
    let workspaces = worktree::load_workspace_views(&state.pool)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(workspaces))
}

pub(super) async fn get_workspace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<worktree::WorkspaceView>> {
    let workspace = worktree::load_workspace_view(&state.pool, id)
        .await
        .map_err(|_| ApiError::NotFound)?;
    Ok(Json(workspace))
}

#[derive(Deserialize)]
pub(super) struct DeleteWorkspaceQuery {
    force: Option<bool>,
    delete_branch: Option<bool>,
}

pub(super) async fn delete_workspace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<DeleteWorkspaceQuery>,
) -> ApiResult<StatusCode> {
    let node_id = node_proxy::workspace_node(&state, id).await?;
    node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::WorkspaceDelete,
        serde_json::to_value((
            ResourceRequest { id },
            worktree::DeleteWorkspaceOptions {
                force: q.force.unwrap_or(false),
                delete_branch: q.delete_branch.unwrap_or(true),
            },
        ))
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn post_workspace_refresh(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let node_id = node_proxy::workspace_node(&state, id).await?;
    node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::WorkspaceRefresh,
        serde_json::to_value(WorkspacePathRequest {
            workspace_id: id,
            path: None,
            all: false,
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn get_workspace_dirty_paths(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<worktree::WorkspaceDirtyPaths>> {
    let node_id = node_proxy::workspace_node(&state, id).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::WorkspaceDirtyPaths,
        serde_json::to_value(WorkspacePathRequest {
            workspace_id: id,
            path: None,
            all: false,
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    Ok(Json(
        serde_json::from_value(result).map_err(anyhow::Error::from)?,
    ))
}

pub(super) async fn get_workspace_files(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<FilesQuery>,
) -> ApiResult<Json<fs_workspace::DirListing>> {
    let node_id = node_proxy::workspace_node(&state, id).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::WorkspaceFiles,
        serde_json::to_value(WorkspacePathRequest {
            workspace_id: id,
            path: q.path,
            all: q.all.unwrap_or(false),
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    Ok(Json(
        serde_json::from_value(result).map_err(anyhow::Error::from)?,
    ))
}

pub(super) async fn get_workspace_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<FileResponse>> {
    let node_id = node_proxy::workspace_node(&state, id).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::WorkspaceFilePreview,
        serde_json::to_value(WorkspacePathRequest {
            workspace_id: id,
            path: Some(q.path),
            all: false,
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    Ok(Json(
        serde_json::from_value(result).map_err(anyhow::Error::from)?,
    ))
}

/// Cognito-authenticated raw bytes for a workspace file — the worktree
/// counterpart to `get_repo_file_raw`. See that handler for the LAN /
/// reverse-proxy rationale.
pub(super) async fn get_workspace_file_raw(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<FileQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let node_id = node_proxy::workspace_node(&state, id).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::WorkspaceFileRaw,
        serde_json::to_value(WorkspacePathRequest {
            workspace_id: id,
            path: Some(q.path),
            all: false,
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    let raw: RawFileResponse = serde_json::from_value(result).map_err(anyhow::Error::from)?;
    let path = raw.path.clone();
    file_content::serve_loaded_bytes(
        path,
        raw.into_bytes().map_err(ApiError::Internal)?,
        &headers,
    )
}

pub(super) async fn get_workspace_file_trace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<FileTraceResponse>> {
    let node_id = node_proxy::workspace_node(&state, id).await?;
    let preview = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::WorkspaceFilePreview,
        serde_json::to_value(WorkspacePathRequest {
            workspace_id: id,
            path: Some(q.path),
            all: false,
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    let preview: FileResponse = serde_json::from_value(preview).map_err(anyhow::Error::from)?;
    let dirty = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::WorkspaceDirtyPaths,
        serde_json::to_value(WorkspacePathRequest {
            workspace_id: id,
            path: None,
            all: false,
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    let dirty: worktree::WorkspaceDirtyPaths =
        serde_json::from_value(dirty).map_err(anyhow::Error::from)?;
    let workspace = worktree::load_workspace(&state.pool, id)
        .await
        .map_err(|_| ApiError::NotFound)?;
    let touches = ingest::load_repo_file_trace(&state.pool, &workspace.repo_name, &preview.path)
        .await
        .map_err(ApiError::Internal)?;
    let dirty = crate::repo_state::RepoDirtyPaths {
        repo: workspace.repo_name,
        git_revision: dirty.git_revision,
        dirty_by_path: dirty.dirty_by_path,
        diff_stats_by_path: dirty.diff_stats_by_path,
    };
    Ok(Json(file_trace_response(preview.path, dirty, touches)))
}

pub(super) async fn get_workspace_diff(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<DiffResponse>> {
    let node_id = node_proxy::workspace_node(&state, id).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::WorkspaceDiff,
        serde_json::to_value(WorkspacePathRequest {
            workspace_id: id,
            path: q.path,
            all: false,
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    Ok(Json(
        serde_json::from_value(result).map_err(anyhow::Error::from)?,
    ))
}

pub(super) async fn post_workspace_stage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<StageReq>,
) -> ApiResult<StatusCode> {
    let node_id = node_proxy::workspace_node(&state, id).await?;
    node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::WorkspaceStage,
        serde_json::to_value(WorkspaceStageRequest {
            workspace_id: id,
            change: StageRequest {
                path: req.path,
                stage: req.stage,
            },
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn post_workspace_upload(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<UploadQuery>,
    mut multipart: Multipart,
) -> ApiResult<Json<UploadResponse>> {
    let node_id = node_proxy::workspace_node(&state, id).await?;
    let directory = q.path.unwrap_or_default();
    let uploads = read_uploads(&mut multipart, &directory).await?;
    let mut first = None;
    for (path, bytes) in uploads {
        let result = node_proxy::request(
            &state,
            node_id,
            NodeRequestKind::WorkspaceUpload,
            serde_json::to_value(WorkspaceUploadRequest {
                workspace_id: id,
                upload: UploadRequest::new(path, &bytes),
            })
            .map_err(anyhow::Error::from)?,
        )
        .await?;
        if first.is_none() {
            first = Some(serde_json::from_value(result).map_err(anyhow::Error::from)?);
        }
    }
    first
        .map(Json)
        .ok_or_else(|| ApiError::BadRequest("no file field".into()))
}
