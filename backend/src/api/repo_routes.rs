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
use super::routes::{repo_path, repos_root, validate_repo_name, ApiError, ApiResult};
use crate::node_protocol::{NodeOperationKind, NodeRequestKind};
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
    if state.node_protocol_required {
        let node_id = node_proxy::default_node(&state).await?;
        let result = node_proxy::operation(
            &state,
            node_id,
            &format!("repo-create:{name}:{}", Uuid::new_v4()),
            NodeOperationKind::RepoCreate,
            None,
            serde_json::to_value(RepoCreateRequest {
                name,
                git_url: req.git_url,
            })
            .map_err(anyhow::Error::from)?,
        )
        .await?;
        let repo = serde_json::from_value(result).map_err(anyhow::Error::from)?;
        return Ok((StatusCode::CREATED, Json(repo)));
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
    if state.node_protocol_required {
        let node_id = node_proxy::repo_node(&state, &name).await?;
        node_proxy::request(
            &state,
            node_id,
            NodeRequestKind::RepoRefresh,
            None,
            serde_json::to_value(RepoPathRequest {
                repo: name,
                path: None,
                all: false,
            })
            .map_err(anyhow::Error::from)?,
        )
        .await?;
        return Ok(StatusCode::ACCEPTED);
    }
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
    if state.node_protocol_required {
        let node_id = node_proxy::repo_node(&state, &name).await?;
        let result = node_proxy::request(
            &state,
            node_id,
            NodeRequestKind::RepoDirtyPaths,
            None,
            serde_json::to_value(RepoPathRequest {
                repo: name,
                path: None,
                all: false,
            })
            .map_err(anyhow::Error::from)?,
        )
        .await?;
        return Ok(Json(
            serde_json::from_value(result).map_err(anyhow::Error::from)?,
        ));
    }
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
    if state.node_protocol_required {
        let node_id = node_proxy::repo_node(&state, &name).await?;
        let result = node_proxy::request(
            &state,
            node_id,
            NodeRequestKind::RepoFiles,
            None,
            serde_json::to_value(RepoPathRequest {
                repo: name,
                path: q.path,
                all: q.all.unwrap_or(false),
            })
            .map_err(anyhow::Error::from)?,
        )
        .await?;
        return Ok(Json(
            serde_json::from_value(result).map_err(anyhow::Error::from)?,
        ));
    }
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
    if state.node_protocol_required {
        let node_id = node_proxy::repo_node(&state, &name).await?;
        let result = node_proxy::request(
            &state,
            node_id,
            NodeRequestKind::RepoFilePreview,
            None,
            serde_json::to_value(RepoPathRequest {
                repo: name,
                path: Some(q.path),
                all: false,
            })
            .map_err(anyhow::Error::from)?,
        )
        .await?;
        return Ok(Json(
            serde_json::from_value(result).map_err(anyhow::Error::from)?,
        ));
    }
    let root = repo_path(&state, &name)?;
    Ok(Json(file_content::build_preview(root, &q.path).await?))
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
    if state.node_protocol_required {
        let node_id = node_proxy::repo_node(&state, &name).await?;
        let result = node_proxy::request(
            &state,
            node_id,
            NodeRequestKind::RepoFileRaw,
            None,
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
        return file_content::serve_loaded_bytes(
            path,
            raw.into_bytes().map_err(ApiError::Internal)?,
            &headers,
        );
    }
    let root = repo_path(&state, &name)?;
    file_content::serve_bytes(root, &q.path, &headers, file_content::RAW_MAX_BYTES).await
}

pub(super) async fn get_repo_file_trace(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<FileTraceResponse>> {
    if state.node_protocol_required {
        let node_id = node_proxy::repo_node(&state, &name).await?;
        let preview = node_proxy::request(
            &state,
            node_id,
            NodeRequestKind::RepoFilePreview,
            None,
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
            None,
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
        return Ok(Json(file_trace_response(preview.path, dirty, touches)));
    }
    let root = repo_path(&state, &name)?;
    let (_, rel) = workspace::resolve_in_repo(&root, &q.path)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let dirty = repo_state::load_dirty_paths(&state.pool, &name)
        .await
        .map_err(ApiError::Internal)?;
    let touches = ingest::load_repo_file_trace(&state.pool, &name, &rel)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(file_trace_response(rel, dirty, touches)))
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
    if state.node_protocol_required {
        let node_id = node_proxy::repo_node(&state, &name).await?;
        let result = node_proxy::request(
            &state,
            node_id,
            NodeRequestKind::RepoDiff,
            None,
            serde_json::to_value(RepoPathRequest {
                repo: name,
                path: q.path,
                all: false,
            })
            .map_err(anyhow::Error::from)?,
        )
        .await?;
        return Ok(Json(
            serde_json::from_value(result).map_err(anyhow::Error::from)?,
        ));
    }
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
    if state.node_protocol_required {
        let node_id = node_proxy::repo_node(&state, &name).await?;
        node_proxy::request(
            &state,
            node_id,
            NodeRequestKind::RepoStage,
            None,
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
        return Ok(StatusCode::NO_CONTENT);
    }
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
    if state.node_protocol_required {
        let node_id = node_proxy::repo_node(&state, &name).await?;
        let dir = q.path.unwrap_or_default();
        let uploads = read_uploads(&mut multipart, &dir).await?;
        let mut first = None;
        for (path, bytes) in uploads {
            let result = node_proxy::request(
                &state,
                node_id,
                NodeRequestKind::RepoUpload,
                None,
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
        return first
            .map(Json)
            .ok_or_else(|| ApiError::BadRequest("no file field".into()));
    }
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
    if state.node_protocol_required {
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
            None,
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
            "repo content ingested through development node",
        );
        return Ok(Json(IngestResponse {
            path: rel,
            bytes: body.len() as u64,
        }));
    }
    let root = repo_path(&state, &name)?;
    let rel = q.path.trim().trim_start_matches('/').to_string();
    if rel.is_empty() {
        return Err(ApiError::BadRequest("path is required".into()));
    }
    let bytes = body.len() as u64;
    if bytes > UPLOAD_MAX_BYTES {
        return Err(ApiError::BadRequest(format!(
            "content exceeds {UPLOAD_MAX_BYTES} bytes"
        )));
    }

    workspace::write_file(root, rel.clone(), body.to_vec())
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    state
        .repo_state
        .request_refresh(&name)
        .await
        .map_err(ApiError::Internal)?;

    tracing::info!(
        repo = %name,
        path = %rel,
        bytes,
        token_id = principal.token_id,
        user = %principal.user_sub,
        "repo content ingested",
    );
    Ok(Json(IngestResponse { path: rel, bytes }))
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
    if state.node_protocol_required {
        let rel = q.path.trim().trim_start_matches('/').to_string();
        if rel.is_empty() {
            return Err(ApiError::BadRequest("path is required".into()));
        }
        let node_id = node_proxy::repo_node(&state, &name).await?;
        let result = node_proxy::request(
            &state,
            node_id,
            NodeRequestKind::RepoFileRaw,
            None,
            serde_json::to_value(RepoPathRequest {
                repo: name,
                path: Some(rel),
                all: false,
            })
            .map_err(anyhow::Error::from)?,
        )
        .await?;
        let raw: RawFileResponse = serde_json::from_value(result).map_err(anyhow::Error::from)?;
        return Ok(Response::builder()
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(raw.into_bytes().map_err(ApiError::Internal)?))
            .expect("octet-stream response is always valid"));
    }
    let root = repo_path(&state, &name)?;
    let rel = q.path.trim().trim_start_matches('/').to_string();
    if rel.is_empty() {
        return Err(ApiError::BadRequest("path is required".into()));
    }
    let (abs, _) =
        workspace::resolve_in_repo(&root, &rel).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let meta = match tokio::fs::metadata(&abs).await {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(ApiError::NotFound),
        Err(err) => return Err(ApiError::Io(err)),
    };
    if !meta.is_file() {
        return Err(ApiError::NotFound);
    }
    let bytes = tokio::fs::read(&abs).await?;

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(bytes))
        .expect("octet-stream response is always valid"))
}
