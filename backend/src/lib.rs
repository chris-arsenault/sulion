use std::sync::Arc;
use std::time::Instant;

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;

pub mod canonical;
pub mod config;
pub mod correlate;
pub mod db;
pub mod emulator;
pub mod git;
pub mod ingester;
pub mod pty;
pub mod routes;
pub mod search;
pub mod stats;
pub mod workspace;
pub mod ws;

#[derive(Clone)]
pub struct AppState {
    pub pool: db::Pool,
    pub pty: Arc<pty::PtyManager>,
    pub repos_root: std::path::PathBuf,
    /// Shared with the background ingester task so the `/api/stats`
    /// handler can read its cumulative counters.
    pub ingester: Arc<ingester::Ingester>,
    /// Timestamp the app was constructed. Surfaced as `uptime_seconds`.
    pub start_time: Instant,
    /// sysinfo probe; holds its own `System` so CPU% diffs work across calls.
    pub stats_probe: Arc<stats::StatsProbe>,
}

impl AppState {
    pub fn new(
        pool: db::Pool,
        repos_root: std::path::PathBuf,
        ingester: Arc<ingester::Ingester>,
    ) -> Arc<Self> {
        let pty = pty::PtyManager::new(pool.clone());
        Arc::new(Self {
            pool,
            pty,
            repos_root,
            ingester,
            start_time: Instant::now(),
            stats_probe: Arc::new(stats::StatsProbe::new()),
        })
    }
}

pub fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/stats", get(stats::stats_handler))
        .route("/api/search", get(search::search_handler))
        .route("/ws/sessions/:id", get(ws::attach))
        .merge(routes::router())
        .with_state(state)
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    db: &'static str,
}

async fn health(State(state): State<Arc<AppState>>) -> (StatusCode, Json<Health>) {
    match db::ping(&state.pool).await {
        Ok(()) => (
            StatusCode::OK,
            Json(Health {
                status: "ok",
                db: "ok",
            }),
        ),
        Err(err) => {
            tracing::error!(error = %err, "db ping failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(Health {
                    status: "degraded",
                    db: "unreachable",
                }),
            )
        }
    }
}

/// Verifies that the embedded migration set parses and is visible to the
/// compile-time migrator. Does not require a running database.
pub fn embedded_migrations_present() -> bool {
    !sqlx::migrate!("./migrations").migrations.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_embedded_at_compile_time() {
        assert!(embedded_migrations_present());
    }
}
