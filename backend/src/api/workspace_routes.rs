//! Workspace-scoped filesystem and git handlers. Repo-scoped routes
//! remain canonical checkout operations; these routes target a specific
//! Sulion workspace/worktree.

use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::repo_routes::{FileResponse, FileTraceResponse, FileTraceTouchResponse};
use super::routes::{ApiError, ApiResult};
use crate::{git, ingest, workspace as fs_workspace, worktree, AppState};

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

#[derive(Serialize)]
pub(super) struct DiffResponse {
    diff: String,
}

#[derive(Deserialize)]
pub(super) struct StageReq {
    path: String,
    stage: bool,
}

#[derive(Serialize)]
pub(super) struct UploadResponse {
    path: String,
    size: u64,
}

#[derive(Deserialize)]
pub(super) struct UploadQuery {
    path: Option<String>,
}

const FILE_PREVIEW_CAP: u64 = 1024 * 1024;
const UPLOAD_MAX_BYTES: u64 = 50 * 1024 * 1024;

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
    state
        .workspace_state
        .delete_workspace(
            id,
            worktree::DeleteWorkspaceOptions {
                force: q.force.unwrap_or(false),
                delete_branch: q.delete_branch.unwrap_or(true),
            },
        )
        .await
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn post_workspace_refresh(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let _ = state
        .workspace_state
        .load_workspace(id)
        .await
        .map_err(|_| ApiError::NotFound)?;
    state
        .workspace_state
        .request_refresh(id)
        .await
        .map_err(ApiError::Internal)?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn get_workspace_dirty_paths(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<worktree::WorkspaceDirtyPaths>> {
    let dirty = worktree::load_workspace_dirty_paths(&state.pool, id)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(dirty))
}

pub(super) async fn get_workspace_files(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<FilesQuery>,
) -> ApiResult<Json<fs_workspace::DirListing>> {
    let workspace = state
        .workspace_state
        .load_workspace(id)
        .await
        .map_err(|_| ApiError::NotFound)?;
    let rel = q.path.unwrap_or_default();
    let only_tracked = !q.all.unwrap_or(false);
    let dirty = worktree::load_workspace_dirty_paths(&state.pool, id)
        .await
        .unwrap_or_else(|_| worktree::WorkspaceDirtyPaths {
            workspace_id: id,
            git_revision: 0,
            dirty_by_path: Default::default(),
            diff_stats_by_path: Default::default(),
        });
    let listing = fs_workspace::list_dir(
        workspace.path,
        rel,
        only_tracked,
        dirty.dirty_by_path,
        dirty.diff_stats_by_path,
    )
    .await
    .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(listing))
}

pub(super) async fn get_workspace_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<FileResponse>> {
    let workspace = state
        .workspace_state
        .load_workspace(id)
        .await
        .map_err(|_| ApiError::NotFound)?;
    let (abs, _) = fs_workspace::resolve_in_repo(&workspace.path, &q.path)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let meta = tokio::fs::metadata(&abs).await?;
    let size = meta.len();
    if size > FILE_PREVIEW_CAP {
        return Ok(Json(FileResponse {
            path: q.path,
            size,
            mime: "application/octet-stream".into(),
            binary: true,
            truncated: true,
            content: None,
        }));
    }
    let bytes = tokio::fs::read(&abs).await?;
    let binary = fs_workspace::looks_binary(&bytes);
    let mime = guess_mime(&q.path, binary);
    let content = if binary {
        None
    } else {
        Some(String::from_utf8_lossy(&bytes).into_owned())
    };
    Ok(Json(FileResponse {
        path: q.path,
        size,
        mime,
        binary,
        truncated: false,
        content,
    }))
}

pub(super) async fn get_workspace_file_trace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<FileTraceResponse>> {
    let workspace = state
        .workspace_state
        .load_workspace(id)
        .await
        .map_err(|_| ApiError::NotFound)?;
    let (_, rel) = fs_workspace::resolve_in_repo(&workspace.path, &q.path)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let dirty = worktree::load_workspace_dirty_paths(&state.pool, id)
        .await
        .map_err(ApiError::Internal)?;
    let touches = ingest::load_repo_file_trace(&state.pool, &workspace.repo_name, &rel)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(FileTraceResponse {
        path: rel.clone(),
        dirty: dirty.dirty_by_path.get(&rel).cloned(),
        current_diff: dirty.diff_stats_by_path.get(&rel).cloned(),
        touches: touches
            .into_iter()
            .map(|touch| FileTraceTouchResponse {
                pty_session_id: touch.pty_session_id,
                session_uuid: touch.session_uuid,
                session_agent: touch.session_agent,
                session_label: touch.session_label,
                session_state: touch.session_state,
                turn_id: touch.turn_id,
                turn_preview: touch.turn_preview,
                turn_timestamp: touch.turn_timestamp,
                operation_type: touch.operation_type,
                operation_category: touch.operation_category,
                pair_id: touch.pair_id,
                touch_kind: touch.touch_kind,
                is_write: touch.is_write,
            })
            .collect(),
    }))
}

pub(super) async fn get_workspace_diff(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<DiffResponse>> {
    let workspace = state
        .workspace_state
        .load_workspace(id)
        .await
        .map_err(|_| ApiError::NotFound)?;
    let diff = git::read_diff(workspace.path, q.path)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    Ok(Json(DiffResponse { diff }))
}

pub(super) async fn post_workspace_stage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<StageReq>,
) -> ApiResult<StatusCode> {
    let workspace = state
        .workspace_state
        .load_workspace(id)
        .await
        .map_err(|_| ApiError::NotFound)?;
    git::stage_path(workspace.path, req.path, req.stage)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    state
        .workspace_state
        .request_refresh(id)
        .await
        .map_err(ApiError::Internal)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn post_workspace_upload(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<UploadQuery>,
    mut multipart: Multipart,
) -> ApiResult<Json<UploadResponse>> {
    let workspace = state
        .workspace_state
        .load_workspace(id)
        .await
        .map_err(|_| ApiError::NotFound)?;
    let dir = q.path.unwrap_or_default();
    let mut first_written: Option<(String, u64)> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart: {e}")))?
    {
        let fname = field
            .file_name()
            .map(|s| s.to_string())
            .ok_or_else(|| ApiError::BadRequest("file field missing filename".into()))?;
        let safe_name = std::path::Path::new(&fname)
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| ApiError::BadRequest("bad filename".into()))?
            .to_string();
        let rel = if dir.is_empty() {
            safe_name.clone()
        } else {
            format!("{}/{}", dir.trim_end_matches('/'), safe_name)
        };

        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| ApiError::BadRequest(format!("multipart read: {e}")))?
        {
            if (buf.len() as u64) + (chunk.len() as u64) > UPLOAD_MAX_BYTES {
                return Err(ApiError::BadRequest(format!(
                    "file exceeds {} bytes",
                    UPLOAD_MAX_BYTES
                )));
            }
            buf.extend_from_slice(&chunk);
        }
        let size = buf.len() as u64;
        let written = fs_workspace::write_file(workspace.path.clone(), rel.clone(), buf)
            .await
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        state
            .workspace_state
            .request_refresh(id)
            .await
            .map_err(ApiError::Internal)?;
        if first_written.is_none() {
            first_written = Some((written.to_string_lossy().into_owned(), size));
        }
    }

    match first_written {
        Some((path, size)) => Ok(Json(UploadResponse { path, size })),
        None => Err(ApiError::BadRequest("no file field".into())),
    }
}

fn guess_mime(path: &str, binary: bool) -> String {
    if binary {
        return "application/octet-stream".into();
    }
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "c" | "cpp" | "h" | "sh"
        | "toml" | "yaml" | "yml" | "html" | "css" | "scss" | "sql" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        _ => "text/plain",
    }
    .to_string()
}
