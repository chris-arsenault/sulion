use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

pub mod agent;
pub mod api;
pub mod auth;
pub mod codex;
pub mod config;
pub mod correlate;
pub mod credential_helper;
pub mod db;
pub mod e2e;
pub mod emulator;
pub mod future_prompts;
pub mod git;
pub mod ingest;
pub mod library;
pub mod pty;
pub mod repo_state;
pub mod secret_broker;
pub mod secret_protocol;
pub mod secret_pty;
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
    pub repo_state: Arc<repo_state::RepoStateManager>,
    /// Shared with the background ingester task so the app-state sampler
    /// can read its runtime totals.
    pub ingester: Arc<ingest::Ingester>,
    /// Timestamp the app was constructed. Surfaced as `uptime_seconds`.
    pub start_time: Instant,
    /// sysinfo probe; holds its own `System` so CPU% diffs work across calls.
    pub stats_probe: Arc<api::StatsProbe>,
    pub stats_cache: Arc<api::StatsCache>,
    /// E2E-only hook that can ask active websocket attachers to close.
    pub ws_test_hooks: Arc<WsTestHooks>,
    /// Optional JWT auth validator. Production wiring enables this;
    /// most unit tests keep it unset and exercise handlers directly.
    pub auth: Option<Arc<auth::AuthState>>,
}

impl AppState {
    pub fn new(
        pool: db::Pool,
        repos_root: std::path::PathBuf,
        library_root: std::path::PathBuf,
        ingester: Arc<ingest::Ingester>,
    ) -> Arc<Self> {
        Self::new_with_auth(pool, repos_root, library_root, ingester, None)
    }

    pub fn new_with_auth(
        pool: db::Pool,
        repos_root: std::path::PathBuf,
        library_root: std::path::PathBuf,
        ingester: Arc<ingest::Ingester>,
        auth: Option<Arc<auth::AuthState>>,
    ) -> Arc<Self> {
        let pty = pty::PtyManager::new(pool.clone());
        let repo_state = repo_state::RepoStateManager::new(pool.clone(), repos_root.clone());
        Arc::new(Self {
            pool,
            pty,
            repos_root,
            library_root,
            repo_state,
            ingester,
            start_time: Instant::now(),
            stats_probe: Arc::new(api::StatsProbe::new()),
            stats_cache: Arc::new(api::StatsCache::new()),
            ws_test_hooks: Arc::new(WsTestHooks::default()),
            auth,
        })
    }
}

pub fn app(state: Arc<AppState>) -> Router {
    api::router(state.clone()).with_state(state)
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
