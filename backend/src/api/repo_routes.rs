//! `/api/repos*` handlers — creation, cached dirty state, files, diff,
//! staging, upload, and file-trace. Timeline for repos lives in
//! `timeline_routes.rs` — keep this module to filesystem + git ops.

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::device_routes::DevicePrincipal;
use super::file_content::{self, FileResponse};
use super::node_proxy;
use super::routes::{validate_repo_name, ApiError, ApiResult};
use crate::node_protocol::NodeRequestKind;
use crate::node_runtime::{
    RawFileResponse, RepoCreateRequest, RepoPathRequest, RepoStageRequest, RepoUploadRequest,
    StageRequest, UploadRequest,
};
use crate::{git, ingest, repo_state, workspace, AppState};

#[derive(Serialize, Deserialize)]
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
    validate_repo_name(&name)?;
    let node_id = node_proxy::default_node(&state).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoCreate,
        serde_json::to_value(RepoCreateRequest {
            name,
            git_url: req.git_url,
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    let repo = serde_json::from_value(result).map_err(anyhow::Error::from)?;
    Ok((StatusCode::CREATED, Json(repo)))
}

pub(super) async fn post_repo_refresh(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    let node_id = node_proxy::repo_node(&state, &name).await?;
    node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoRefresh,
        serde_json::to_value(RepoPathRequest {
            repo: name,
            path: None,
            all: false,
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn get_repo_dirty_paths(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> ApiResult<Json<repo_state::RepoDirtyPaths>> {
    let node_id = node_proxy::repo_node(&state, &name).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoDirtyPaths,
        serde_json::to_value(RepoPathRequest {
            repo: name,
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
    let node_id = node_proxy::repo_node(&state, &name).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoFiles,
        serde_json::to_value(RepoPathRequest {
            repo: name,
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

#[derive(Deserialize)]
pub(super) struct FileQuery {
    path: String,
}

#[derive(Serialize)]
pub(super) struct FileTraceTouchResponse {
    pub(super) pty_session_id: Option<Uuid>,
    pub(super) session_uuid: Uuid,
    pub(super) session_agent: Option<String>,
    pub(super) session_label: Option<String>,
    pub(super) session_state: Option<String>,
    pub(super) turn_id: i64,
    pub(super) turn_preview: String,
    pub(super) turn_timestamp: chrono::DateTime<chrono::Utc>,
    pub(super) operation_type: Option<String>,
    pub(super) operation_category: Option<String>,
    /// Stable id of the tool call this touch belongs to. Null for
    /// touches that aren't attached to a specific tool (e.g. bare
    /// user-prompt turns); callers fall back to turn-level focus.
    pub(super) pair_id: Option<String>,
    pub(super) touch_kind: String,
    pub(super) is_write: bool,
}

#[derive(Serialize)]
pub(super) struct FileTraceResponse {
    pub(super) path: String,
    pub(super) dirty: Option<String>,
    pub(super) current_diff: Option<git::DiffStat>,
    pub(super) touches: Vec<FileTraceTouchResponse>,
}

pub(super) async fn get_repo_file(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<FileResponse>> {
    let node_id = node_proxy::repo_node(&state, &name).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoFilePreview,
        serde_json::to_value(RepoPathRequest {
            repo: name,
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

/// Cognito-authenticated raw bytes for the browser file viewer: images, PDFs,
/// downloads, and any binary the preview route reports with `content: null`.
/// Same-origin under `/api`, so it works identically on the LAN and through the
/// reverse proxy — the browser sends its bearer token like every other call.
pub(super) async fn get_repo_file_raw(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<FileQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let node_id = node_proxy::repo_node(&state, &name).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoFileRaw,
        serde_json::to_value(RepoPathRequest {
            repo: name,
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

pub(super) async fn get_repo_file_trace(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<FileTraceResponse>> {
    let node_id = node_proxy::repo_node(&state, &name).await?;
    let preview = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoFilePreview,
        serde_json::to_value(RepoPathRequest {
            repo: name.clone(),
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
        NodeRequestKind::RepoDirtyPaths,
        serde_json::to_value(RepoPathRequest {
            repo: name.clone(),
            path: None,
            all: false,
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    let dirty: repo_state::RepoDirtyPaths =
        serde_json::from_value(dirty).map_err(anyhow::Error::from)?;
    let touches = ingest::load_repo_file_trace(&state.pool, &name, &preview.path)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(file_trace_response(preview.path, dirty, touches)))
}

pub(super) fn file_trace_response(
    path: String,
    dirty: repo_state::RepoDirtyPaths,
    touches: Vec<ingest::RepoFileTraceTouch>,
) -> FileTraceResponse {
    FileTraceResponse {
        dirty: dirty.dirty_by_path.get(&path).cloned(),
        current_diff: dirty.diff_stats_by_path.get(&path).cloned(),
        path,
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
    }
}

#[derive(Deserialize)]
pub(super) struct DiffQuery {
    path: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct DiffResponse {
    diff: String,
}

pub(super) async fn get_repo_diff(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<DiffResponse>> {
    let node_id = node_proxy::repo_node(&state, &name).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoDiff,
        serde_json::to_value(RepoPathRequest {
            repo: name,
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
    let node_id = node_proxy::repo_node(&state, &name).await?;
    node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoStage,
        serde_json::to_value(RepoStageRequest {
            repo: name,
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

#[derive(Serialize, Deserialize)]
pub(super) struct UploadResponse {
    path: String,
    size: u64,
}

#[derive(Deserialize)]
pub(super) struct UploadQuery {
    path: Option<String>,
}

const UPLOAD_MAX_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB

pub(super) async fn read_uploads(
    multipart: &mut Multipart,
    directory: &str,
) -> ApiResult<Vec<(String, Vec<u8>)>> {
    let mut uploads = Vec::new();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::BadRequest(format!("multipart: {error}")))?
    {
        let filename = field
            .file_name()
            .map(str::to_string)
            .ok_or_else(|| ApiError::BadRequest("file field missing filename".into()))?;
        let safe_name = std::path::Path::new(&filename)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ApiError::BadRequest("bad filename".into()))?;
        let path = if directory.is_empty() {
            safe_name.to_string()
        } else {
            format!("{}/{}", directory.trim_end_matches('/'), safe_name)
        };
        let mut bytes = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|error| ApiError::BadRequest(format!("multipart read: {error}")))?
        {
            if bytes.len() as u64 + chunk.len() as u64 > UPLOAD_MAX_BYTES {
                return Err(ApiError::BadRequest(format!(
                    "file exceeds {UPLOAD_MAX_BYTES} bytes"
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        uploads.push((path, bytes));
    }
    Ok(uploads)
}

pub(super) async fn post_repo_upload(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<UploadQuery>,
    mut multipart: Multipart,
) -> ApiResult<Json<UploadResponse>> {
    let node_id = node_proxy::repo_node(&state, &name).await?;
    let dir = q.path.unwrap_or_default();
    let uploads = read_uploads(&mut multipart, &dir).await?;
    let mut first = None;
    for (path, bytes) in uploads {
        let result = node_proxy::request(
            &state,
            node_id,
            NodeRequestKind::RepoUpload,
            serde_json::to_value(RepoUploadRequest {
                repo: name.clone(),
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

#[derive(Deserialize)]
pub(super) struct IngestQuery {
    /// Repo-relative destination path, e.g. `clips/verse.mid`. Required.
    path: String,
}

#[derive(Serialize)]
pub(super) struct IngestResponse {
    path: String,
    bytes: u64,
}

/// Device-token-authenticated content drop: write the raw request body to
/// `path` (a repo-relative path) under repo `name`, creating parent dirs. This
/// is the HTTP analogue of the terminal's paste-as-file, for external tools
/// (first consumer: the Ableton "Send to Sulion" extension). It's
/// content-agnostic — binary or text, the caller chooses the filename.
///
/// Path safety (no `..`, no absolute, no symlink escape) is enforced by
/// `workspace::write_file`.
pub(super) async fn post_repo_ingest(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<IngestQuery>,
    Extension(principal): Extension<DevicePrincipal>,
    body: Bytes,
) -> ApiResult<Json<IngestResponse>> {
    let rel = q.path.trim().trim_start_matches('/').to_string();
    if rel.is_empty() {
        return Err(ApiError::BadRequest("path is required".into()));
    }
    if body.len() as u64 > UPLOAD_MAX_BYTES {
        return Err(ApiError::BadRequest(format!(
            "content exceeds {UPLOAD_MAX_BYTES} bytes"
        )));
    }
    let node_id = node_proxy::repo_node(&state, &name).await?;
    node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoUpload,
        serde_json::to_value(RepoUploadRequest {
            repo: name.clone(),
            upload: UploadRequest::new(rel.clone(), &body),
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    tracing::info!(
        repo = %name,
        path = %rel,
        bytes = body.len(),
        token_id = principal.token_id,
        user = %principal.user_sub,
        "repo content ingested",
    );
    Ok(Json(IngestResponse {
        path: rel,
        bytes: body.len() as u64,
    }))
}

#[derive(Deserialize)]
pub(super) struct RawQuery {
    /// Repo-relative source path, e.g. `clips/verse.mid`. Required.
    path: String,
}

/// Device-token-authenticated raw file read: return the bytes at `path` under
/// repo `name` as `application/octet-stream`. The counterpart to
/// `post_repo_ingest`, for pulling content (e.g. a generated `.mid`) back out
/// to an external tool. `GET /api/repos/:name/file` can't serve this — it's
/// Cognito-only and nulls binary content. Path-safety (no `..`/absolute/symlink
/// escape) via `workspace::resolve_in_repo`.
pub(super) async fn get_repo_raw(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<RawQuery>,
    Extension(_principal): Extension<DevicePrincipal>,
) -> ApiResult<Response> {
    let rel = q.path.trim().trim_start_matches('/').to_string();
    if rel.is_empty() {
        return Err(ApiError::BadRequest("path is required".into()));
    }
    let node_id = node_proxy::repo_node(&state, &name).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoFileRaw,
        serde_json::to_value(RepoPathRequest {
            repo: name,
            path: Some(rel),
            all: false,
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    let raw: RawFileResponse = serde_json::from_value(result).map_err(anyhow::Error::from)?;
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(raw.into_bytes().map_err(ApiError::Internal)?))
        .expect("octet-stream response is always valid"))
}
