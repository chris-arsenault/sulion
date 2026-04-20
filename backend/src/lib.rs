use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

pub mod agent;
pub mod api;
pub mod codex;
pub mod config;
pub mod correlate;
pub mod db;
pub mod e2e;
pub mod emulator;
pub mod git;
pub mod ingest;
pub mod library;
pub mod pty;
pub mod workspace;

#[derive(Default)]
pub struct WsTestHooks {
    sessions: RwLock<std::collections::HashMap<Uuid, broadcast::Sender<()>>>,
}

impl WsTestHooks {
    pub async fn subscribe(&self, session_id: Uuid) -> broadcast::Receiver<()> {
        let mut sessions = self.sessions.write().await;
        sessions
            .entry(session_id)
            .or_insert_with(|| broadcast::channel(8).0)
            .subscribe()
    }

    pub async fn drop_live_ws(&self, session_id: Uuid) -> bool {
        let sessions = self.sessions.read().await;
        let Some(tx) = sessions.get(&session_id) else {
            return false;
        };
        tx.send(()).is_ok()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub pool: db::Pool,
    pub pty: Arc<pty::PtyManager>,
    pub repos_root: std::path::PathBuf,
    pub library_root: std::path::PathBuf,
    /// Shared with the background ingester task so the `/api/stats`
    /// handler can read its cumulative counters.
    pub ingester: Arc<ingest::Ingester>,
    /// Timestamp the app was constructed. Surfaced as `uptime_seconds`.
    pub start_time: Instant,
    /// sysinfo probe; holds its own `System` so CPU% diffs work across calls.
    pub stats_probe: Arc<api::StatsProbe>,
    /// E2E-only hook that can ask active websocket attachers to close.
    pub ws_test_hooks: Arc<WsTestHooks>,
}

impl AppState {
    pub fn new(
        pool: db::Pool,
        repos_root: std::path::PathBuf,
        library_root: std::path::PathBuf,
        ingester: Arc<ingest::Ingester>,
    ) -> Arc<Self> {
        let pty = pty::PtyManager::new(pool.clone());
        Arc::new(Self {
            pool,
            pty,
            repos_root,
            library_root,
            ingester,
            start_time: Instant::now(),
            stats_probe: Arc::new(api::StatsProbe::new()),
            ws_test_hooks: Arc::new(WsTestHooks::default()),
        })
    }
}

pub fn app(state: Arc<AppState>) -> Router {
    api::router().with_state(state)
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
