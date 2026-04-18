//! REST API handlers. Everything here reads from Postgres — no JSONL
//! file I/O lives in the request path. The ingester owns the JSONL
//! boundary (see `ingester.rs`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pty::{self, PtyMetadata, SpawnParams};
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    use axum::routing::{delete, get};
    Router::new()
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/:id", delete(delete_session))
        .route("/api/sessions/:id/history", get(session_history))
        .route("/api/repos", get(list_repos).post(create_repo))
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
}

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
    let (shell, args) = match req.claude_resume_uuid {
        Some(uuid) => (
            PathBuf::from("/bin/bash"),
            vec![
                "-c".to_string(),
                format!("claude --resume {uuid} ; exec bash"),
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
    payload: serde_json::Value,
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
    let rows: Vec<(
        i64,
        chrono::DateTime<chrono::Utc>,
        String,
        serde_json::Value,
    )> = if let Some(kind) = &q.kind {
        sqlx::query_as(
            "SELECT byte_offset, timestamp, kind, payload \
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
            "SELECT byte_offset, timestamp, kind, payload \
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

    let next_after = rows.last().map(|(o, _, _, _)| *o);
    let events = rows
        .into_iter()
        .map(|(byte_offset, timestamp, kind, payload)| EventView {
            byte_offset,
            timestamp,
            kind,
            payload,
        })
        .collect();

    Ok(Json(HistoryResponse {
        claude_session_uuid: Some(claude_uuid),
        events,
        next_after,
    }))
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

// ─── helpers ─────────────────────────────────────────────────────────

fn repos_root(state: &AppState) -> ApiResult<PathBuf> {
    Ok(state.repos_root.clone())
}
