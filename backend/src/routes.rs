//! REST API handlers. Everything here reads from Postgres — no JSONL
//! file I/O lives in the request path. The ingester owns the JSONL
//! boundary (see `ingester.rs`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pty::{self, PtyMetadata, SpawnParams};
use crate::{git, workspace, AppState};

pub fn router() -> Router<Arc<AppState>> {
    use axum::routing::{delete, get, post};
    Router::new()
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route(
            "/api/sessions/:id",
            delete(delete_session).patch(patch_session),
        )
        .route("/api/sessions/:id/history", get(session_history))
        .route("/api/repos", get(list_repos).post(create_repo))
        .route("/api/repos/:name/git", get(get_repo_git))
        .route("/api/repos/:name/git/diff", get(get_repo_diff))
        .route("/api/repos/:name/git/stage", post(post_repo_stage))
        .route("/api/repos/:name/files", get(get_repo_files))
        .route("/api/repos/:name/file", get(get_repo_file))
        .route("/api/repos/:name/upload", post(post_repo_upload))
}

// ─── error type ───────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("internal: {0}")]
    Internal(#[from] anyhow::Error),
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            ApiError::Internal(e) => {
                tracing::error!(%e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
            ApiError::Db(e) => {
                tracing::error!(%e, "db error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "database error".to_string(),
                )
            }
            ApiError::Io(e) => {
                tracing::error!(%e, "io error");
                (StatusCode::INTERNAL_SERVER_ERROR, "io error".to_string())
            }
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

// ─── sessions ────────────────────────────────────────────────────────

#[derive(Serialize)]
struct SessionView {
    id: Uuid,
    repo: String,
    working_dir: String,
    state: &'static str,
    created_at: chrono::DateTime<chrono::Utc>,
    ended_at: Option<chrono::DateTime<chrono::Utc>>,
    exit_code: Option<i32>,
    current_claude_session_uuid: Option<Uuid>,
    /// MAX(event.timestamp) for this session's current Claude UUID.
    /// Null means no events ingested yet. Used by the frontend's
    /// unread-dot indicator.
    last_event_at: Option<chrono::DateTime<chrono::Utc>>,
    /// User-facing label; overrides the uuid prefix in the sidebar.
    label: Option<String>,
    /// Pinned sessions float to the top of their repo group.
    pinned: bool,
    /// Palette-constrained colour tag name.
    color: Option<String>,
}

/// Allowed palette names for session colour tags. The backend rejects
/// anything outside this set so invalid strings don't sneak into the
/// UI and produce unstyled chips.
const COLOR_PALETTE: &[&str] = &[
    "amber", "emerald", "sky", "rose", "violet", "slate", "teal", "fuchsia",
];

impl From<PtyMetadata> for SessionView {
    fn from(m: PtyMetadata) -> Self {
        let state = match m.state {
            pty::PtyState::Live => "live",
            pty::PtyState::Dead => "dead",
            pty::PtyState::Deleted => "deleted",
            pty::PtyState::Orphaned => "orphaned",
        };
        Self {
            id: m.id,
            repo: m.repo,
            working_dir: m.working_dir.to_string_lossy().into_owned(),
            state,
            created_at: m.created_at,
            ended_at: m.ended_at,
            exit_code: m.exit_code,
            current_claude_session_uuid: m.current_claude_session_uuid,
            last_event_at: m.last_event_at,
            label: m.label,
            pinned: m.pinned,
            color: m.color,
        }
    }
}

#[derive(Serialize)]
struct ListSessionsResponse {
    sessions: Vec<SessionView>,
}

async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ListSessionsResponse>> {
    let metas = state.pty.list().await?;
    Ok(Json(ListSessionsResponse {
        sessions: metas.into_iter().map(SessionView::from).collect(),
    }))
}

#[derive(Deserialize)]
struct CreateSessionReq {
    repo: String,
    working_dir: Option<String>,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
    /// If set, the shell boots straight into `claude --resume <uuid>`
    /// and falls back to an interactive bash after Claude exits. Used
    /// by the "Resume" action on orphaned sessions.
    #[serde(default)]
    claude_resume_uuid: Option<Uuid>,
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSessionReq>,
) -> ApiResult<(StatusCode, Json<SessionView>)> {
    if req.repo.is_empty() {
        return Err(ApiError::BadRequest("repo must not be empty".into()));
    }

    // Resolve working directory. If absent, use <repos_root>/<repo>.
    let repos_root = repos_root(&state)?;
    let working_dir = match &req.working_dir {
        Some(p) => PathBuf::from(p),
        None => repos_root.join(&req.repo),
    };
    if !working_dir.exists() {
        return Err(ApiError::BadRequest(format!(
            "working dir does not exist: {}",
            working_dir.display()
        )));
    }

    // When resuming a prior Claude session, boot bash directly into the
    // resume command and fall back to an interactive shell after. The
    // uuid is `Uuid`-typed so no shell injection is possible.
    // `--dangerously-skip-permissions` matches the user's default
    // workflow (same flag as the `cl` alias).
    let (shell, args) = match req.claude_resume_uuid {
        Some(uuid) => (
            PathBuf::from("/bin/bash"),
            vec![
                "-c".to_string(),
                format!("claude --dangerously-skip-permissions --resume {uuid} ; exec bash"),
            ],
        ),
        None => (pty::default_shell(), Vec::new()),
    };

    let params = SpawnParams {
        repo: req.repo.clone(),
        working_dir,
        shell,
        args,
        cols: req.cols.unwrap_or(120),
        rows: req.rows.unwrap_or(32),
    };
    let meta = state.pty.spawn(params).await?;
    Ok((StatusCode::CREATED, Json(SessionView::from(meta))))
}

async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    state.pty.delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct PatchSessionReq {
    /// Set the label. Empty string clears. Null/absent = no change.
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    pinned: Option<bool>,
    /// One of COLOR_PALETTE names, or empty string to clear. Null =
    /// no change.
    #[serde(default)]
    color: Option<String>,
}

async fn patch_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchSessionReq>,
) -> ApiResult<StatusCode> {
    if let Some(name) = &req.color {
        if !name.is_empty() && !COLOR_PALETTE.contains(&name.as_str()) {
            return Err(ApiError::BadRequest(format!(
                "color must be one of: {}",
                COLOR_PALETTE.join(", "),
            )));
        }
    }
    if let Some(label) = &req.label {
        if label.len() > 100 {
            return Err(ApiError::BadRequest(
                "label must be 100 characters or fewer".into(),
            ));
        }
    }
    state
        .pty
        .update_metadata(id, req.label.map(Some), req.pinned, req.color.map(Some))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── history ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct HistoryQuery {
    #[serde(default)]
    after: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    claude_session: Option<Uuid>,
}

#[derive(Serialize)]
struct EventView {
    byte_offset: i64,
    timestamp: chrono::DateTime<chrono::Utc>,
    kind: String,
    /// Raw JSONL payload. Kept for forensic / debug use and for agents
    /// the parser doesn't understand yet. New code should read `blocks`
    /// instead.
    payload: serde_json::Value,
    agent: String,
    speaker: Option<String>,
    content_kind: Option<String>,
    /// Canonical content blocks, agent-agnostic. Empty for unparsable
    /// events or those still waiting on the startup backfill.
    blocks: Vec<crate::canonical::Block>,
}

#[derive(Serialize)]
struct HistoryResponse {
    claude_session_uuid: Option<Uuid>,
    events: Vec<EventView>,
    next_after: Option<i64>,
}

async fn session_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<HistoryResponse>> {
    // Figure out which claude session to read from.
    let claude_uuid = match q.claude_session {
        Some(u) => Some(u),
        None => {
            // Fall back to pty's current pointer.
            let row: Option<(Option<Uuid>,)> = sqlx::query_as(
                "SELECT current_claude_session_uuid FROM pty_sessions WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
            match row {
                Some((Some(u),)) => Some(u),
                Some((None,)) => None,
                None => return Err(ApiError::NotFound),
            }
        }
    };

    let Some(claude_uuid) = claude_uuid else {
        return Ok(Json(HistoryResponse {
            claude_session_uuid: None,
            events: Vec::new(),
            next_after: None,
        }));
    };

    let after = q.after.unwrap_or(-1);
    let limit = q.limit.unwrap_or(500).clamp(1, 5000);

    // Build query with optional kind filter. Using a CASE keeps things
    // simple without query builders — the bound params are positional.
    type HistoryRow = (
        i64,
        chrono::DateTime<chrono::Utc>,
        String,
        serde_json::Value,
        String,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<HistoryRow> = if let Some(kind) = &q.kind {
        sqlx::query_as(
            "SELECT byte_offset, timestamp, kind, payload, agent, speaker, content_kind \
                 FROM events \
                 WHERE session_uuid = $1 AND byte_offset > $2 AND kind = $3 \
                 ORDER BY byte_offset ASC \
                 LIMIT $4",
        )
        .bind(claude_uuid)
        .bind(after)
        .bind(kind)
        .bind(limit)
        .fetch_all(&state.pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT byte_offset, timestamp, kind, payload, agent, speaker, content_kind \
                 FROM events \
                 WHERE session_uuid = $1 AND byte_offset > $2 \
                 ORDER BY byte_offset ASC \
                 LIMIT $3",
        )
        .bind(claude_uuid)
        .bind(after)
        .bind(limit)
        .fetch_all(&state.pool)
        .await?
    };

    let next_after = rows.last().map(|r| r.0);

    // Load canonical blocks for this event window in one query.
    let blocks_by_offset =
        load_event_blocks(&state.pool, claude_uuid, rows.iter().map(|r| r.0)).await?;

    let events = rows
        .into_iter()
        .map(
            |(byte_offset, timestamp, kind, payload, agent, speaker, content_kind)| {
                let blocks = blocks_by_offset
                    .get(&byte_offset)
                    .cloned()
                    .unwrap_or_default();
                EventView {
                    byte_offset,
                    timestamp,
                    kind,
                    payload,
                    agent,
                    speaker,
                    content_kind,
                    blocks,
                }
            },
        )
        .collect();

    Ok(Json(HistoryResponse {
        claude_session_uuid: Some(claude_uuid),
        events,
        next_after,
    }))
}

/// Fetch `event_blocks` for a set of byte offsets within one claude
/// session, grouped by byte_offset and sorted by `ord`. Returns a map
/// so the caller can match them back to each event row without another
/// round-trip.
async fn load_event_blocks(
    pool: &crate::db::Pool,
    session_uuid: Uuid,
    offsets: impl Iterator<Item = i64>,
) -> ApiResult<std::collections::HashMap<i64, Vec<crate::canonical::Block>>> {
    let list: Vec<i64> = offsets.collect();
    if list.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    #[derive(sqlx::FromRow)]
    struct Row {
        byte_offset: i64,
        ord: i32,
        kind: String,
        text: Option<String>,
        tool_id: Option<String>,
        tool_name: Option<String>,
        tool_name_canonical: Option<String>,
        tool_input: Option<serde_json::Value>,
        is_error: Option<bool>,
        raw: Option<serde_json::Value>,
    }

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT byte_offset, ord, kind, text, tool_id, tool_name, \
                tool_name_canonical, tool_input, is_error, raw \
           FROM event_blocks \
          WHERE session_uuid = $1 AND byte_offset = ANY($2) \
          ORDER BY byte_offset ASC, ord ASC",
    )
    .bind(session_uuid)
    .bind(&list)
    .fetch_all(pool)
    .await?;

    let mut out: std::collections::HashMap<i64, Vec<crate::canonical::Block>> =
        std::collections::HashMap::new();
    for r in rows {
        let kind = match r.kind.as_str() {
            "text" => crate::canonical::BlockKind::Text,
            "thinking" => crate::canonical::BlockKind::Thinking,
            "tool_use" => crate::canonical::BlockKind::ToolUse,
            "tool_result" => crate::canonical::BlockKind::ToolResult,
            _ => crate::canonical::BlockKind::Unknown,
        };
        out.entry(r.byte_offset)
            .or_default()
            .push(crate::canonical::Block {
                ord: r.ord,
                kind,
                text: r.text,
                tool_id: r.tool_id,
                tool_name: r.tool_name,
                tool_name_canonical: r.tool_name_canonical,
                tool_input: r.tool_input,
                is_error: r.is_error,
                raw: r.raw,
            });
    }
    Ok(out)
}

// ─── repos ───────────────────────────────────────────────────────────

#[derive(Serialize)]
struct RepoView {
    name: String,
    path: String,
}

#[derive(Serialize)]
struct ListReposResponse {
    repos: Vec<RepoView>,
}

async fn list_repos(State(state): State<Arc<AppState>>) -> ApiResult<Json<ListReposResponse>> {
    let root = repos_root(&state)?;
    let mut repos = Vec::new();
    if !root.exists() {
        return Ok(Json(ListReposResponse { repos }));
    }
    let mut entries = tokio::fs::read_dir(&root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        repos.push(RepoView {
            name: name.clone(),
            path: entry.path().to_string_lossy().into_owned(),
        });
    }
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(ListReposResponse { repos }))
}

#[derive(Deserialize)]
struct CreateRepoReq {
    name: String,
    /// Optional git URL to clone. If absent, we `git init` an empty dir.
    git_url: Option<String>,
}

async fn create_repo(
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

    Ok((
        StatusCode::CREATED,
        Json(RepoView {
            name,
            path: dest.to_string_lossy().into_owned(),
        }),
    ))
}

// ─── repo git / files / diff / upload ─────────────────────────────────

fn repo_path(state: &AppState, name: &str) -> ApiResult<PathBuf> {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return Err(ApiError::BadRequest("invalid repo name".into()));
    }
    let root = repos_root(state)?;
    let p = root.join(name);
    if !p.is_dir() {
        return Err(ApiError::NotFound);
    }
    Ok(p)
}

async fn get_repo_git(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> ApiResult<Json<git::GitStatus>> {
    let path = repo_path(&state, &name)?;
    let status = git::read_status(path)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    Ok(Json(status))
}

#[derive(Deserialize)]
struct FilesQuery {
    path: Option<String>,
    all: Option<bool>,
}

async fn get_repo_files(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<FilesQuery>,
) -> ApiResult<Json<workspace::DirListing>> {
    let path = repo_path(&state, &name)?;
    let rel = q.path.unwrap_or_default();
    let only_tracked = !q.all.unwrap_or(false);
    // Pull the dirty map so the listing carries sigils without a round
    // trip. Cheap — already the status endpoint's payload.
    let status = git::read_status(path.clone()).await.unwrap_or_default();
    let listing = workspace::list_dir(path, rel, only_tracked, status.dirty_by_path)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(listing))
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}

#[derive(Serialize)]
struct FileResponse {
    path: String,
    size: u64,
    mime: String,
    binary: bool,
    truncated: bool,
    content: Option<String>,
}

const FILE_PREVIEW_CAP: u64 = 1024 * 1024; // 1 MiB

async fn get_repo_file(
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
struct DiffQuery {
    path: Option<String>,
}

#[derive(Serialize)]
struct DiffResponse {
    diff: String,
}

async fn get_repo_diff(
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
struct StageReq {
    path: String,
    stage: bool,
}

async fn post_repo_stage(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(req): Json<StageReq>,
) -> ApiResult<StatusCode> {
    let path = repo_path(&state, &name)?;
    git::stage_path(path, req.path, req.stage)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct UploadResponse {
    path: String,
    size: u64,
}

const UPLOAD_MAX_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB

async fn post_repo_upload(
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
struct UploadQuery {
    path: Option<String>,
}

// ─── helpers ─────────────────────────────────────────────────────────

fn repos_root(state: &AppState) -> ApiResult<PathBuf> {
    Ok(state.repos_root.clone())
}
