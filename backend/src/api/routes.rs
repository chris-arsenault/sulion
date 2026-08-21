//! REST routing hub. The individual handlers live in sibling modules
//! (`session_routes`, `repo_routes`, `plan_routes`, `library_routes`,
//! `timeline_routes`, `future_prompt_routes`); this file only wires
//! the URL layout, owns the `ApiError` type shared across them, and
//! carries a couple of helpers used by more than one module.
//!
//! The ingester is the sole JSONL reader (see `crate::ingest::ingester`) —
//! every handler under this tree reads from Postgres or the filesystem
//! under the user's repo/library roots, never the raw transcript.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};

use super::{
    admin_routes, future_prompt_routes, library_routes, meta_repo_routes, plan_routes,
    repo_lifecycle_routes, repo_routes, session_routes, timeline_routes, workspace_routes,
};
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    use axum::routing::{delete, get, patch, post};
    Router::new()
        .route("/api/sessions", post(session_routes::create_session))
        .route(
            "/api/sessions/:id",
            delete(session_routes::delete_session).patch(session_routes::patch_session),
        )
        .route(
            "/api/sessions/:id/upgrade",
            post(session_routes::upgrade_session),
        )
        .route(
            "/api/sessions/:id/agent",
            post(session_routes::start_session_agent),
        )
        .route(
            "/api/sessions/:id/agent/interrupt",
            post(session_routes::interrupt_session_agent),
        )
        .route(
            "/api/sessions/:id/prompt",
            post(session_routes::send_session_prompt),
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
            "/api/sessions/:id/timeline/turns/:turn_id",
            get(timeline_routes::session_timeline_turn),
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
        .route("/api/repos", post(repo_routes::create_repo))
        .route("/api/meta-repos", post(meta_repo_routes::create_meta_repo))
        .route(
            "/api/meta-repos/:id",
            axum::routing::put(meta_repo_routes::update_meta_repo)
                .delete(meta_repo_routes::delete_meta_repo),
        )
        .route(
            "/api/repos/:name",
            patch(repo_lifecycle_routes::patch_repo).delete(repo_lifecycle_routes::delete_repo),
        )
        .route(
            "/api/repos/:name/timeline",
            get(timeline_routes::repo_timeline),
        )
        .route(
            "/api/repos/:name/timeline/turns/:session_uuid/:turn_id",
            get(timeline_routes::repo_timeline_turn),
        )
        .merge(plan_router())
        .route(
            "/api/monitor/timeline",
            post(timeline_routes::monitor_timeline),
        )
        .route("/api/metrics", get(portfolio_metrics))
        .route("/api/jobs", get(background_jobs))
        .merge(workspace_router())
        .route("/api/repos/:name/git/diff", get(repo_routes::get_repo_diff))
        .route(
            "/api/repos/:name/refresh",
            post(repo_routes::post_repo_refresh),
        )
        .route(
            "/api/repos/:name/dirty-paths",
            get(repo_routes::get_repo_dirty_paths),
        )
        .route(
            "/api/repos/:name/git/stage",
            post(repo_routes::post_repo_stage),
        )
        .route("/api/repos/:name/files", get(repo_routes::get_repo_files))
        .route("/api/repos/:name/file", get(repo_routes::get_repo_file))
        .route(
            "/api/repos/:name/file/raw",
            get(repo_routes::get_repo_file_raw),
        )
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
        .route(
            "/api/admin/retrieval/reindex",
            post(admin_routes::retrieval_reindex),
        )
}

fn workspace_router() -> Router<Arc<AppState>> {
    use axum::routing::{get, post};

    Router::new()
        .route("/api/workspaces", get(workspace_routes::list_workspaces))
        .route(
            "/api/workspaces/:id",
            get(workspace_routes::get_workspace).delete(workspace_routes::delete_workspace),
        )
        .route(
            "/api/workspaces/:id/refresh",
            post(workspace_routes::post_workspace_refresh),
        )
        .route(
            "/api/workspaces/:id/dirty-paths",
            get(workspace_routes::get_workspace_dirty_paths),
        )
        .route(
            "/api/workspaces/:id/files",
            get(workspace_routes::get_workspace_files),
        )
        .route(
            "/api/workspaces/:id/file",
            get(workspace_routes::get_workspace_file),
        )
        .route(
            "/api/workspaces/:id/file/raw",
            get(workspace_routes::get_workspace_file_raw),
        )
        .route(
            "/api/workspaces/:id/file-trace",
            get(workspace_routes::get_workspace_file_trace),
        )
        .route(
            "/api/workspaces/:id/git/diff",
            get(workspace_routes::get_workspace_diff),
        )
        .route(
            "/api/workspaces/:id/git/stage",
            post(workspace_routes::post_workspace_stage),
        )
        .route(
            "/api/workspaces/:id/upload",
            post(workspace_routes::post_workspace_upload),
        )
}

fn plan_router() -> Router<Arc<AppState>> {
    use axum::routing::{delete, get, patch, post};

    Router::new()
        .route(
            "/api/repos/:name/plans",
            get(plan_routes::list_repo_plans).post(plan_routes::create_repo_plan),
        )
        .route(
            "/api/plans/:id",
            get(plan_routes::get_plan).patch(plan_routes::patch_plan),
        )
        .route("/api/plans/:id/phases", post(plan_routes::add_plan_phase))
        .route(
            "/api/plans/:id/phases/:phase_id",
            patch(plan_routes::patch_plan_phase),
        )
        .route("/api/plans/:id/attachments", post(plan_routes::attach_plan))
        .route(
            "/api/plans/:id/attachments/:pty_session_id",
            delete(plan_routes::detach_plan),
        )
        .route("/api/plans/:id/events", get(plan_routes::plan_events))
}

/// Portfolio metrics for the monitor and metrics tab: token rollups,
/// git activity, churn hotspots, and plan flow. Read-only aggregation;
/// git scans are cached in-process.
async fn portfolio_metrics(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<crate::metrics::MetricsResponse>> {
    Ok(Json(crate::metrics::portfolio_metrics(&state.pool).await?))
}

/// Active and recently finished background jobs (startup backfills,
/// transcript catch-up) for the jobs panel.
async fn background_jobs(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<crate::ingest::jobs::JobsResponse>> {
    Ok(Json(crate::ingest::jobs::list_jobs(&state.pool).await?))
}

// ─── error type ───────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unavailable: {0}")]
    Unavailable(String),
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
            ApiError::Unavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m.clone()),
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
    validate_repo_name(name)?;
    let root = repos_root(state)?;
    let p = root.join(name);
    if !p.is_dir() {
        return Err(ApiError::NotFound);
    }
    Ok(p)
}

pub(super) fn validate_repo_name(name: &str) -> ApiResult<()> {
    if !crate::workspace::is_valid_repo_name(name) {
        return Err(ApiError::BadRequest("invalid repo name".into()));
    }
    Ok(())
}

pub(super) fn repos_root(state: &AppState) -> ApiResult<PathBuf> {
    Ok(state.repos_root.clone())
}
