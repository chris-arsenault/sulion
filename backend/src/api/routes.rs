//! REST routing hub. The individual handlers live in sibling modules
//! (`session_routes`, `repo_routes`, `library_routes`,
//! `timeline_routes`, `future_prompt_routes`); this file only wires
//! the URL layout, owns the `ApiError` type shared across them, and
//! carries a couple of helpers used by more than one module.
//!
//! The ingester is the sole JSONL reader (see `crate::ingest::ingester`) —
//! every handler under this tree reads from Postgres or the filesystem
//! under the user's repo/library roots, never the raw transcript.

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};

use super::{
    admin_routes, future_prompt_routes, library_routes, repo_routes, session_routes,
    timeline_routes,
};
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    use axum::routing::{delete, get, post};
    Router::new()
        .route(
            "/api/sessions",
            get(session_routes::list_sessions).post(session_routes::create_session),
        )
        .route(
            "/api/sessions/:id",
            delete(session_routes::delete_session).patch(session_routes::patch_session),
        )
        .route(
            "/api/sessions/:id/e2e/drop-ws",
            post(session_routes::drop_session_ws),
        )
        .route(
            "/api/sessions/:id/history",
            get(session_routes::session_history),
        )
        .route(
            "/api/sessions/:id/timeline",
            get(timeline_routes::session_timeline),
        )
        .route(
            "/api/sessions/:id/future-prompts",
            get(future_prompt_routes::list_future_prompts)
                .put(future_prompt_routes::create_future_prompt),
        )
        .route(
            "/api/sessions/:id/future-prompts/:item_id",
            delete(future_prompt_routes::delete_future_prompt)
                .patch(future_prompt_routes::update_future_prompt),
        )
        .route(
            "/api/repos",
            get(repo_routes::list_repos).post(repo_routes::create_repo),
        )
        .route(
            "/api/repos/:name/timeline",
            get(timeline_routes::repo_timeline),
        )
        .route("/api/repos/:name/git", get(repo_routes::get_repo_git))
        .route("/api/repos/:name/git/diff", get(repo_routes::get_repo_diff))
        .route(
            "/api/repos/:name/git/stage",
            post(repo_routes::post_repo_stage),
        )
        .route("/api/repos/:name/files", get(repo_routes::get_repo_files))
        .route("/api/repos/:name/file", get(repo_routes::get_repo_file))
        .route(
            "/api/repos/:name/file-trace",
            get(repo_routes::get_repo_file_trace),
        )
        .route(
            "/api/repos/:name/upload",
            post(repo_routes::post_repo_upload),
        )
        .route(
            "/api/library/:kind",
            get(library_routes::list_library).put(library_routes::put_library_root),
        )
        .route(
            "/api/library/:kind/:slug",
            get(library_routes::get_library_entry)
                .put(library_routes::put_library_entry)
                .delete(library_routes::delete_library_entry),
        )
        .route("/api/admin/reindex", post(admin_routes::reindex))
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

pub(super) type ApiResult<T> = Result<T, ApiError>;

// ─── shared helpers ──────────────────────────────────────────────────

pub(super) fn repo_path(state: &AppState, name: &str) -> ApiResult<PathBuf> {
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

pub(super) fn repos_root(state: &AppState) -> ApiResult<PathBuf> {
    Ok(state.repos_root.clone())
}
