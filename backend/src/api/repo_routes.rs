//! `/api/repos*` handlers — creation, cached dirty state, files, diff,
//! staging, upload, and file-trace. Timeline for repos lives in
//! `timeline_routes.rs` — keep this module to filesystem + git ops.

use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::routes::{repo_path, repos_root, ApiError, ApiResult};
use crate::{git, ingest, repo_state, workspace, AppState};

#[derive(Serialize)]
pub(super) struct RepoView {
    name: String,
    path: String,
}

#[derive(Deserialize)]
pub(super) struct CreateRepoReq {
    name: String,
    /// Optional git URL to clone. If absent, we `git init` an empty dir.
    git_url: Option<String>,
}

pub(super) async fn create_repo(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateRepoReq>,
) -> ApiResult<(StatusCode, Json<RepoView>)> {
    let name = req.name.trim().to_string();
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return Err(ApiError::BadRequest("invalid repo name".into()));
    }
    let root = repos_root(&state)?;
    tokio::fs::create_dir_all(&root).await?;
    let dest = root.join(&name);
    if dest.exists() {
        return Err(ApiError::BadRequest(format!(
            "repo already exists: {}",
            dest.display()
        )));
    }

    if let Some(url) = &req.git_url {
        let out = tokio::process::Command::new("git")
            .arg("clone")
            .arg(url)
            .arg(&dest)
            .output()
            .await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            // Echo the URL back so the caller can see exactly what
            // reached git — rules out form-level mangling during
            // diagnosis.
            return Err(ApiError::BadRequest(format!(
                "git clone of {url:?} failed: {stderr}"
            )));
        }
    } else {
        tokio::fs::create_dir_all(&dest).await?;
        let out = tokio::process::Command::new("git")
            .arg("init")
            .arg(&dest)
            .output()
            .await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            return Err(ApiError::Internal(anyhow::anyhow!(
                "git init failed: {stderr}"
            )));
        }
    }

    state
        .repo_state
        .upsert_repo(&name, &dest)
        .await
        .map_err(ApiError::Internal)?;

    Ok((
        StatusCode::CREATED,
        Json(RepoView {
            name,
            path: dest.to_string_lossy().into_owned(),
        }),
    ))
}

pub(super) async fn post_repo_refresh(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    let _ = repo_path(&state, &name)?;
    state
        .repo_state
        .request_refresh(&name)
        .await
        .map_err(ApiError::Internal)?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn get_repo_dirty_paths(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> ApiResult<Json<repo_state::RepoDirtyPaths>> {
    let _ = repo_path(&state, &name)?;
    let dirty = repo_state::load_dirty_paths(&state.pool, &name)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(dirty))
}

#[derive(Deserialize)]
pub(super) struct FilesQuery {
    path: Option<String>,
    all: Option<bool>,
}

pub(super) async fn get_repo_files(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<FilesQuery>,
) -> ApiResult<Json<workspace::DirListing>> {
    let path = repo_path(&state, &name)?;
    let rel = q.path.unwrap_or_default();
    let only_tracked = !q.all.unwrap_or(false);
    let dirty = repo_state::load_dirty_paths(&state.pool, &name)
        .await
        .unwrap_or_else(|_| repo_state::RepoDirtyPaths {
            repo: name.clone(),
            git_revision: 0,
            dirty_by_path: Default::default(),
            diff_stats_by_path: Default::default(),
        });
    let listing = workspace::list_dir(
        path,
        rel,
        only_tracked,
        dirty.dirty_by_path,
        dirty.diff_stats_by_path,
    )
    .await
    .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(listing))
}

#[derive(Deserialize)]
pub(super) struct FileQuery {
    path: String,
}

#[derive(Serialize)]
pub(super) struct FileResponse {
    path: String,
    size: u64,
    mime: String,
    binary: bool,
    truncated: bool,
    content: Option<String>,
}

#[derive(Serialize)]
pub(super) struct FileTraceTouchResponse {
    pty_session_id: Option<Uuid>,
    session_uuid: Uuid,
    session_agent: Option<String>,
    session_label: Option<String>,
    session_state: Option<String>,
    turn_id: i64,
    turn_preview: String,
    turn_timestamp: chrono::DateTime<chrono::Utc>,
    operation_type: Option<String>,
    operation_category: Option<String>,
    /// Stable id of the tool call this touch belongs to. Null for
    /// touches that aren't attached to a specific tool (e.g. bare
    /// user-prompt turns); callers fall back to turn-level focus.
    pair_id: Option<String>,
    touch_kind: String,
    is_write: bool,
}

#[derive(Serialize)]
pub(super) struct FileTraceResponse {
    path: String,
    dirty: Option<String>,
    current_diff: Option<git::DiffStat>,
    touches: Vec<FileTraceTouchResponse>,
}

const FILE_PREVIEW_CAP: u64 = 1024 * 1024; // 1 MiB

pub(super) async fn get_repo_file(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<FileResponse>> {
    let root = repo_path(&state, &name)?;
    let (abs, _) = workspace::resolve_in_repo(&root, &q.path)
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
    let binary = workspace::looks_binary(&bytes);
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

pub(super) async fn get_repo_file_trace(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<FileTraceResponse>> {
    let root = repo_path(&state, &name)?;
    let (_, rel) = workspace::resolve_in_repo(&root, &q.path)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let dirty = repo_state::load_dirty_paths(&state.pool, &name)
        .await
        .map_err(ApiError::Internal)?;
    let touches = ingest::load_repo_file_trace(&state.pool, &name, &rel)
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

#[derive(Deserialize)]
pub(super) struct DiffQuery {
    path: Option<String>,
}

#[derive(Serialize)]
pub(super) struct DiffResponse {
    diff: String,
}

pub(super) async fn get_repo_diff(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<DiffResponse>> {
    let path = repo_path(&state, &name)?;
    let diff = git::read_diff(path, q.path)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    Ok(Json(DiffResponse { diff }))
}

#[derive(Deserialize)]
pub(super) struct StageReq {
    path: String,
    stage: bool,
}

pub(super) async fn post_repo_stage(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(req): Json<StageReq>,
) -> ApiResult<StatusCode> {
    let path = repo_path(&state, &name)?;
    git::stage_path(path, req.path, req.stage)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    state
        .repo_state
        .request_refresh(&name)
        .await
        .map_err(ApiError::Internal)?;
    Ok(StatusCode::NO_CONTENT)
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

const UPLOAD_MAX_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB

pub(super) async fn post_repo_upload(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<UploadQuery>,
    mut multipart: Multipart,
) -> ApiResult<Json<UploadResponse>> {
    let root = repo_path(&state, &name)?;
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
        // Reject anything with a path in the filename — we honour the
        // directory the user dropped onto, not the one the browser
        // encoded into the form.
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
        let written = workspace::write_file(root.clone(), rel.clone(), buf)
            .await
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        state
            .repo_state
            .request_refresh(&name)
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
