//! REST API handlers. Everything here reads from Postgres — no JSONL
//! file I/O lives in the request path. The ingester owns the JSONL
//! boundary (see `crate::ingest::ingester`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ingest::{self, canonical, timeline};
use crate::library::{self, LibraryKind};
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
        .route("/api/sessions/:id/e2e/drop-ws", post(drop_session_ws))
        .route("/api/sessions/:id/history", get(session_history))
        .route("/api/sessions/:id/timeline", get(session_timeline))
        .route("/api/repos", get(list_repos).post(create_repo))
        .route("/api/repos/:name/git", get(get_repo_git))
        .route("/api/repos/:name/git/diff", get(get_repo_diff))
        .route("/api/repos/:name/git/stage", post(post_repo_stage))
        .route("/api/repos/:name/files", get(get_repo_files))
        .route("/api/repos/:name/file", get(get_repo_file))
        .route("/api/repos/:name/file-trace", get(get_repo_file_trace))
        .route("/api/repos/:name/upload", post(post_repo_upload))
        .route(
            "/api/library/:kind",
            get(list_library).put(put_library_root),
        )
        .route(
            "/api/library/:kind/:slug",
            get(get_library_entry)
                .put(put_library_entry)
                .delete(delete_library_entry),
        )
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
    current_session_uuid: Option<Uuid>,
    current_session_agent: Option<String>,
    /// MAX(event.timestamp) for this session's current transcript session.
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
            current_session_uuid: m.current_session_uuid,
            current_session_agent: m.current_session_agent,
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
    /// If set, the shell boots straight into the agent-specific resume
    /// command and falls back to an interactive shell after.
    #[serde(default)]
    resume_session_uuid: Option<Uuid>,
    #[serde(default)]
    resume_agent: Option<String>,
    /// Backward-compatible alias for older frontend builds.
    #[serde(default)]
    claude_resume_uuid: Option<Uuid>,
    /// Test-only scripted fixture. Rejected unless the backend was
    /// started with `SULION_ENABLE_E2E_FIXTURES=1`.
    #[serde(default)]
    e2e_fixture: Option<String>,
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

    // When resuming a prior agent session, boot bash directly into the
    // agent-specific resume command and fall back to an interactive
    // shell after. The uuid is `Uuid`-typed so no shell injection is
    // possible.
    let e2e_fixture = req
        .e2e_fixture
        .as_deref()
        .map(str::trim)
        .filter(|fixture| !fixture.is_empty());
    let resume_session_uuid = req.resume_session_uuid.or(req.claude_resume_uuid);
    let resume_agent = req
        .resume_agent
        .as_deref()
        .or_else(|| resume_session_uuid.map(|_| "claude-code"));
    if e2e_fixture.is_some() && resume_session_uuid.is_some() {
        return Err(ApiError::BadRequest(
            "e2e_fixture cannot be combined with resume_session_uuid".into(),
        ));
    }
    let (shell, args) = match (e2e_fixture, resume_session_uuid, resume_agent) {
        (Some(crate::e2e::MOCK_TERMINAL_FIXTURE), None, _) => {
            if !crate::e2e::fixtures_enabled() {
                return Err(ApiError::BadRequest(
                    "e2e fixtures are disabled on this backend".into(),
                ));
            }
            let path = crate::e2e::mock_terminal_script_path(&repos_root);
            if !path.is_file() {
                return Err(ApiError::Internal(anyhow::anyhow!(
                    "mock terminal fixture missing: {}",
                    path.display()
                )));
            }
            (path, Vec::new())
        }
        (Some(fixture), None, _) => {
            return Err(ApiError::BadRequest(format!(
                "unknown e2e fixture {fixture}",
            )));
        }
        (Some(_), Some(_), _) => unreachable!("fixture + resume handled above"),
        (None, Some(uuid), Some("claude-code")) => (
            PathBuf::from("/bin/bash"),
            vec![
                "-c".to_string(),
                format!(
                    "{} agent-launcher --type claude --mode real -- --dangerously-skip-permissions --resume {uuid} ; exec bash",
                    shell_quote(&crate::agent::binary_path())
                ),
            ],
        ),
        (None, Some(uuid), Some("codex")) => (
            PathBuf::from("/bin/bash"),
            vec![
                "-c".to_string(),
                format!(
                    "{} agent-launcher --type codex --mode real -- --yolo resume {uuid} ; exec bash",
                    shell_quote(&crate::agent::binary_path())
                ),
            ],
        ),
        (None, Some(_), Some(agent)) => {
            return Err(ApiError::BadRequest(format!(
                "resume is not implemented for agent {agent}",
            )));
        }
        (None, Some(_), None) => {
            return Err(ApiError::BadRequest(
                "resume_agent is required when resume_session_uuid is set".into(),
            ));
        }
        (None, None, _) => (pty::default_shell(), Vec::new()),
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
    session: Option<Uuid>,
    #[serde(default)]
    claude_session: Option<Uuid>,
}

#[derive(Deserialize)]
struct TimelineQuery {
    #[serde(default)]
    session: Option<Uuid>,
    #[serde(default)]
    claude_session: Option<Uuid>,
    #[serde(default)]
    hide_speakers: Option<String>,
    #[serde(default)]
    hide_categories: Option<String>,
    #[serde(default)]
    errors_only: Option<bool>,
    #[serde(default)]
    show_bookkeeping: Option<bool>,
    #[serde(default)]
    show_sidechain: Option<bool>,
    #[serde(default)]
    file_path: Option<String>,
}

#[derive(Serialize)]
struct EventView {
    byte_offset: i64,
    timestamp: chrono::DateTime<chrono::Utc>,
    kind: String,
    agent: String,
    speaker: Option<String>,
    content_kind: Option<String>,
    event_uuid: Option<String>,
    parent_event_uuid: Option<String>,
    related_tool_use_id: Option<String>,
    is_sidechain: bool,
    is_meta: bool,
    subtype: Option<String>,
    /// Canonical content blocks, agent-agnostic. Empty for unparsable
    /// events or those still waiting on the startup backfill.
    blocks: Vec<canonical::Block>,
}

#[derive(Serialize)]
struct HistoryResponse {
    session_uuid: Option<Uuid>,
    session_agent: Option<String>,
    events: Vec<EventView>,
    next_after: Option<i64>,
}

async fn session_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<HistoryResponse>> {
    let resolved =
        timeline::resolve_session_target(&state.pool, id, q.session.or(q.claude_session)).await?;

    let resolved = match resolved {
        timeline::SessionLookup::Resolved(resolved) => resolved,
        timeline::SessionLookup::NoSession => {
            return Ok(Json(HistoryResponse {
                session_uuid: None,
                session_agent: None,
                events: Vec::new(),
                next_after: None,
            }));
        }
        timeline::SessionLookup::MissingPty => return Err(ApiError::NotFound),
    };
    let events = timeline::load_session_events(
        &state.pool,
        resolved.session_uuid,
        &timeline::SessionEventFilter {
            after: q.after,
            limit: Some(q.limit.unwrap_or(5000)),
            kind: q.kind.clone(),
        },
    )
    .await?;

    let next_after = events.last().map(|event| event.byte_offset);

    let events = events
        .into_iter()
        .map(|event| EventView {
            byte_offset: event.byte_offset,
            timestamp: event.timestamp,
            kind: event.kind,
            agent: event.agent,
            speaker: event.speaker,
            content_kind: event.content_kind,
            event_uuid: event.event_uuid,
            parent_event_uuid: event.parent_event_uuid,
            related_tool_use_id: event.related_tool_use_id,
            is_sidechain: event.is_sidechain,
            is_meta: event.is_meta,
            subtype: event.subtype,
            blocks: event.blocks,
        })
        .collect();

    Ok(Json(HistoryResponse {
        session_uuid: Some(resolved.session_uuid),
        session_agent: resolved.session_agent,
        events,
        next_after,
    }))
}

async fn session_timeline(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Json<timeline::TimelineResponse>> {
    let resolved =
        timeline::resolve_session_target(&state.pool, id, q.session.or(q.claude_session)).await?;

    let resolved = match resolved {
        timeline::SessionLookup::Resolved(resolved) => resolved,
        timeline::SessionLookup::NoSession => {
            return Ok(Json(timeline::TimelineResponse {
                session_uuid: None,
                session_agent: None,
                total_event_count: 0,
                turns: Vec::new(),
            }));
        }
        timeline::SessionLookup::MissingPty => return Err(ApiError::NotFound),
    };

    let filters = timeline::ProjectionFilters {
        hidden_speakers: parse_hidden_speakers(q.hide_speakers.as_deref()),
        hidden_operation_categories: parse_hidden_categories(q.hide_categories.as_deref()),
        errors_only: q.errors_only.unwrap_or(false),
        show_bookkeeping: q.show_bookkeeping.unwrap_or(false),
        show_sidechain: q.show_sidechain.unwrap_or(false),
        file_path: q.file_path.unwrap_or_default(),
    };

    let mut response =
        ingest::load_timeline_response(&state.pool, resolved.session_uuid, &filters).await?;
    response.session_uuid = Some(resolved.session_uuid);
    response.session_agent = resolved.session_agent;
    Ok(Json(response))
}

fn parse_hidden_speakers(raw: Option<&str>) -> std::collections::HashSet<timeline::SpeakerFacet> {
    let mut out = std::collections::HashSet::new();
    for value in raw.unwrap_or_default().split(',').map(str::trim) {
        match value {
            "user" => {
                out.insert(timeline::SpeakerFacet::User);
            }
            "assistant" => {
                out.insert(timeline::SpeakerFacet::Assistant);
            }
            "tool_result" => {
                out.insert(timeline::SpeakerFacet::ToolResult);
            }
            _ => {}
        }
    }
    out
}

fn parse_hidden_categories(
    raw: Option<&str>,
) -> std::collections::HashSet<canonical::OperationCategory> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter_map(canonical::OperationCategory::parse)
        .collect()
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
    let listing = workspace::list_dir(
        path,
        rel,
        only_tracked,
        status.dirty_by_path,
        status.diff_stats_by_path,
    )
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

#[derive(Serialize)]
struct FileTraceTouchResponse {
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
    touch_kind: String,
    is_write: bool,
}

#[derive(Serialize)]
struct FileTraceResponse {
    path: String,
    dirty: Option<String>,
    current_diff: Option<git::DiffStat>,
    touches: Vec<FileTraceTouchResponse>,
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

async fn get_repo_file_trace(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<FileTraceResponse>> {
    let root = repo_path(&state, &name)?;
    let (_, rel) = workspace::resolve_in_repo(&root, &q.path)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let status = git::read_status(root)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    let touches = ingest::load_repo_file_trace(&state.pool, &name, &rel)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(FileTraceResponse {
        path: rel.clone(),
        dirty: status.dirty_by_path.get(&rel).cloned(),
        current_diff: status.diff_stats_by_path.get(&rel).cloned(),
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

async fn drop_session_ws(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    if !crate::e2e::fixtures_enabled() {
        return Err(ApiError::NotFound);
    }
    if state.ws_test_hooks.drop_live_ws(id).await {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[derive(Deserialize)]
struct UploadQuery {
    path: Option<String>,
}

// ─── global library (references + prompts) ──────────────────────────

fn parse_kind(s: &str) -> ApiResult<LibraryKind> {
    LibraryKind::parse(s).ok_or_else(|| ApiError::BadRequest(format!("unknown library kind: {s}")))
}

async fn list_library(
    State(state): State<Arc<AppState>>,
    Path(kind): Path<String>,
) -> ApiResult<Json<Vec<library::LibraryEntry>>> {
    let kind = parse_kind(&kind)?;
    let entries = library::list(&state.library_root, kind)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    Ok(Json(entries))
}

async fn get_library_entry(
    State(state): State<Arc<AppState>>,
    Path((kind, slug)): Path<(String, String)>,
) -> ApiResult<Json<library::LibraryEntry>> {
    let kind = parse_kind(&kind)?;
    let entry = library::read(&state.library_root, kind, &slug)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(entry))
}

/// POST/PUT to the root URL: derive a slug from the name. Returns the
/// saved entry (including the derived slug) so the client can round-trip.
async fn put_library_root(
    State(state): State<Arc<AppState>>,
    Path(kind): Path<String>,
    Json(input): Json<library::SaveInput>,
) -> ApiResult<Json<library::LibraryEntry>> {
    let kind = parse_kind(&kind)?;
    let desired_slug = library::sanitise_slug(&input.name)
        .ok_or_else(|| ApiError::BadRequest("name must derive a valid slug".into()))?;
    let slug = library::next_available_slug(&state.library_root, kind, &desired_slug);
    let entry = library::save(&state.library_root, kind, &slug, input)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    Ok(Json(entry))
}

async fn put_library_entry(
    State(state): State<Arc<AppState>>,
    Path((kind, slug)): Path<(String, String)>,
    Json(input): Json<library::SaveInput>,
) -> ApiResult<Json<library::LibraryEntry>> {
    let kind = parse_kind(&kind)?;
    let entry = library::save(&state.library_root, kind, &slug, input)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    Ok(Json(entry))
}

async fn delete_library_entry(
    State(state): State<Arc<AppState>>,
    Path((kind, slug)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let kind = parse_kind(&kind)?;
    let removed = library::delete(&state.library_root, kind, &slug)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    Ok(if removed {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    })
}

// ─── helpers ─────────────────────────────────────────────────────────

fn repos_root(state: &AppState) -> ApiResult<PathBuf> {
    Ok(state.repos_root.clone())
}

fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}
