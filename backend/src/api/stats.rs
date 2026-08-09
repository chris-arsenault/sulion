//! Lightweight resource snapshot for the unified `/api/app-state` surface
//! (ticket #27). Answers "is this deploy sized correctly?" without
//! ssh-ing to the host. Not a replacement for Grafana; no history,
//! no alerting, no per-session attribution.
//!
//! Memory/CPU describes the development node, not this process: the node is
//! where PTYs, builds, language servers, and development containers run, so
//! its machine is the one that runs out of headroom. The node reports it on
//! its heartbeat and the control plane keeps the latest sample. Database size
//! plus a few inventory counts come from lightweight Postgres queries. A
//! background sampler owns the cadence; app-state only reads the cached
//! sample.

use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::RwLock;

use crate::node_protocol::{NodeDevenvStatus, NodeHostStats};
use crate::AppState;

const RUNTIME_STATS_INTERVAL: Duration = Duration::from_secs(10);
const DATABASE_INVENTORY_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Serialize)]
pub struct StatsResponse {
    pub uptime_seconds: u64,
    /// None until a node has sent one heartbeat, and again once it
    /// disconnects. The browser renders the absence rather than a stale
    /// number.
    pub node: Option<NodeHostStats>,
    /// The node's PTY path: whether the devenv new sessions route to is
    /// dialed in. Same lifecycle as `node`; also None from a node release
    /// that predates the field.
    pub devenv: Option<NodeDevenvStatus>,
    pub pty: PtyStats,
    pub db: DbStats,
    pub ingest: IngestStats,
    pub inventory: InventoryStats,
}

#[derive(Clone, Serialize)]
pub struct PtyStats {
    /// Live PTYs currently tracked by the backend process.
    pub live_sessions: usize,
    /// Live PTYs with a correlated current transcript session.
    pub live_agent_sessions: i64,
}

#[derive(Clone, Serialize)]
pub struct DbStats {
    pub database_size_bytes: i64,
}

#[derive(Clone, Serialize)]
pub struct IngestStats {
    pub last_tick_started_at_unix: Option<i64>,
    pub last_progress_at_unix: Option<i64>,
    pub stalled_seconds: Option<i64>,
}

#[derive(Clone, Serialize)]
pub struct InventoryStats {
    pub event_rows: i64,
    pub agent_sessions: i64,
    pub pty_sessions: i64,
    pub tracked_files: i64,
    pub events_inserted_since_boot: u64,
    pub parse_errors_since_boot: u64,
}

#[derive(Clone, Copy, sqlx::FromRow)]
struct RuntimeStatsSnapshot {
    live_pty_sessions: i64,
    live_agent_sessions: i64,
    pty_sessions: i64,
    tracked_files: i64,
}

#[derive(Clone, Copy, sqlx::FromRow)]
struct DatabaseInventorySnapshot {
    database_size_bytes: i64,
    event_rows: i64,
    agent_sessions: i64,
}

#[derive(Default)]
pub struct StatsCache {
    inner: RwLock<Option<StatsResponse>>,
    database_inventory: RwLock<Option<DatabaseInventorySnapshot>>,
}

impl StatsCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self) -> Option<StatsResponse> {
        self.inner.read().await.clone()
    }

    async fn store(&self, stats: StatsResponse) {
        *self.inner.write().await = Some(stats);
    }

    async fn database_inventory(&self) -> Option<DatabaseInventorySnapshot> {
        *self.database_inventory.read().await
    }

    async fn store_database_inventory(&self, inventory: DatabaseInventorySnapshot) {
        *self.database_inventory.write().await = Some(inventory);
    }
}

async fn collect_stats_with_database_refresh(
    state: &AppState,
    refresh_database_inventory: bool,
) -> anyhow::Result<StatsResponse> {
    let uptime_seconds = state.start_time.elapsed().as_secs();
    let node = state.node_control.host_stats().await;
    let devenv = state.node_control.devenv_status().await;
    let runtime = runtime_stats_snapshot(&state.pool).await?;
    let database = if refresh_database_inventory {
        let snapshot = database_inventory_snapshot(&state.pool).await?;
        state.stats_cache.store_database_inventory(snapshot).await;
        snapshot
    } else if let Some(snapshot) = state.stats_cache.database_inventory().await {
        snapshot
    } else {
        let snapshot = database_inventory_snapshot(&state.pool).await?;
        state.stats_cache.store_database_inventory(snapshot).await;
        snapshot
    };
    let pty = PtyStats {
        live_sessions: runtime.live_pty_sessions.max(0) as usize,
        live_agent_sessions: runtime.live_agent_sessions,
    };
    let db = DbStats {
        database_size_bytes: database.database_size_bytes,
    };
    let now_unix = chrono::Utc::now().timestamp();
    let ingest = IngestStats {
        last_tick_started_at_unix: state.ingester.last_tick_started_at_unix(),
        last_progress_at_unix: state.ingester.last_progress_at_unix(),
        stalled_seconds: state
            .ingester
            .last_progress_at_unix()
            .map(|ts| now_unix.saturating_sub(ts)),
    };
    let inventory = InventoryStats {
        event_rows: database.event_rows,
        agent_sessions: database.agent_sessions,
        pty_sessions: runtime.pty_sessions,
        tracked_files: runtime.tracked_files,
        events_inserted_since_boot: state.ingester.events_inserted_total(),
        parse_errors_since_boot: state.ingester.parse_errors_total(),
    };
    Ok(StatsResponse {
        uptime_seconds,
        node,
        devenv,
        pty,
        db,
        ingest,
        inventory,
    })
}

pub async fn sample_stats_once(state: &AppState) -> anyhow::Result<()> {
    sample_stats(state, true).await
}

async fn sample_stats(state: &AppState, refresh_database_inventory: bool) -> anyhow::Result<()> {
    let stats = collect_stats_with_database_refresh(state, refresh_database_inventory).await?;
    state.stats_cache.store(stats).await;
    Ok(())
}

#[cfg(feature = "integration-tests")]
pub async fn sample_runtime_stats_once_for_tests(state: &AppState) -> anyhow::Result<()> {
    sample_stats(state, false).await
}

pub async fn run_stats_sampler(state: std::sync::Arc<AppState>) {
    let mut last_database_sample = None;
    loop {
        let refresh_database_inventory = last_database_sample
            .map(|sampled_at: Instant| sampled_at.elapsed() >= DATABASE_INVENTORY_INTERVAL)
            .unwrap_or(true);
        if let Err(err) = sample_stats(&state, refresh_database_inventory).await {
            tracing::warn!(%err, "stats sample failed");
        } else if refresh_database_inventory {
            last_database_sample = Some(Instant::now());
        }
        tokio::time::sleep(RUNTIME_STATS_INTERVAL).await;
    }
}

async fn runtime_stats_snapshot(pool: &crate::db::Pool) -> sqlx::Result<RuntimeStatsSnapshot> {
    sqlx::query_as(
        "SELECT
            (SELECT COUNT(*)::BIGINT
               FROM pty_sessions
              WHERE state = 'live') AS live_pty_sessions,
            (SELECT COUNT(*)::BIGINT
               FROM pty_sessions
              WHERE state = 'live' AND current_session_uuid IS NOT NULL) AS live_agent_sessions,
            (SELECT COUNT(*)::BIGINT FROM pty_sessions) AS pty_sessions,
            (SELECT COUNT(*)::BIGINT FROM ingester_state) AS tracked_files",
    )
    .fetch_one(pool)
    .await
}

async fn database_inventory_snapshot(
    pool: &crate::db::Pool,
) -> sqlx::Result<DatabaseInventorySnapshot> {
    sqlx::query_as(
        "SELECT
            pg_database_size(current_database())::BIGINT AS database_size_bytes,
            (SELECT COUNT(*)::BIGINT FROM events) AS event_rows,
            (SELECT COUNT(*)::BIGINT FROM claude_sessions) AS agent_sessions",
    )
    .fetch_one(pool)
    .await
}
