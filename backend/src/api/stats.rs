//! Lightweight resource snapshot for the unified `/api/app-state` surface
//! (ticket #27). Answers "is this deploy sized correctly?" without
//! ssh-ing to the host. Not a replacement for Grafana; no history,
//! no alerting, no per-session attribution.
//!
//! Process memory/CPU comes from `sysinfo`; database size plus a few
//! inventory counts come from lightweight Postgres queries. A background
//! sampler owns the cadence; app-state only reads the cached sample.

use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
use tokio::sync::{Mutex, RwLock};

use crate::AppState;

#[derive(Clone, Serialize)]
pub struct StatsResponse {
    pub uptime_seconds: u64,
    pub process: ProcessStats,
    pub pty: PtyStats,
    pub db: DbStats,
    pub ingest: IngestStats,
    pub inventory: InventoryStats,
}

#[derive(Clone, Serialize)]
pub struct ProcessStats {
    pub memory_rss_bytes: u64,
    pub cpu_percent: f32,
    /// Cgroup v2 memory ceiling when readable (`/sys/fs/cgroup/memory.max`).
    /// Null on hosts without cgroups or when the value is "max" (no limit).
    pub memory_limit_bytes: Option<u64>,
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

#[derive(sqlx::FromRow)]
struct StatsSnapshot {
    database_size_bytes: i64,
    live_agent_sessions: i64,
    event_rows: i64,
    agent_sessions: i64,
    pty_sessions: i64,
    tracked_files: i64,
}

/// Shared state for the stats handler. Holds a `sysinfo::System`
/// instance so successive calls can diff CPU time (first sample always
/// returns 0% CPU).
pub struct StatsProbe {
    sys: Mutex<System>,
    pid: Pid,
}

impl StatsProbe {
    pub fn new() -> Self {
        let pid = Pid::from_u32(std::process::id());
        let sys = System::new_with_specifics(
            RefreshKind::new().with_processes(ProcessRefreshKind::new().with_cpu().with_memory()),
        );
        Self {
            sys: Mutex::new(sys),
            pid,
        }
    }

    pub async fn sample(&self) -> ProcessStats {
        let mut sys = self.sys.lock().await;
        sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::new().with_cpu().with_memory(),
        );
        let proc = sys.process(self.pid);
        ProcessStats {
            memory_rss_bytes: proc.map(|p| p.memory()).unwrap_or(0),
            cpu_percent: proc.map(|p| p.cpu_usage()).unwrap_or(0.0),
            memory_limit_bytes: read_cgroup_memory_max(),
        }
    }
}

impl Default for StatsProbe {
    fn default() -> Self {
        Self::new()
    }
}

/// Cgroup v2 memory ceiling. Returns None on hosts where the file is
/// absent or when the value is "max" (unlimited).
fn read_cgroup_memory_max() -> Option<u64> {
    let raw = std::fs::read_to_string("/sys/fs/cgroup/memory.max").ok()?;
    let trimmed = raw.trim();
    if trimmed == "max" {
        return None;
    }
    trimmed.parse::<u64>().ok()
}

#[derive(Default)]
pub struct StatsCache {
    inner: RwLock<Option<StatsResponse>>,
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
}

pub async fn collect_stats(state: &AppState) -> anyhow::Result<StatsResponse> {
    let uptime_seconds = state.start_time.elapsed().as_secs();
    let process = state.stats_probe.sample().await;
    let snapshot = stats_snapshot(&state.pool).await?;
    let pty = PtyStats {
        live_sessions: state.pty.live_count().await,
        live_agent_sessions: snapshot.live_agent_sessions,
    };
    let db = DbStats {
        database_size_bytes: snapshot.database_size_bytes,
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
        event_rows: snapshot.event_rows,
        agent_sessions: snapshot.agent_sessions,
        pty_sessions: snapshot.pty_sessions,
        tracked_files: snapshot.tracked_files,
        events_inserted_since_boot: state.ingester.events_inserted_total(),
        parse_errors_since_boot: state.ingester.parse_errors_total(),
    };
    Ok(StatsResponse {
        uptime_seconds,
        process,
        pty,
        db,
        ingest,
        inventory,
    })
}

pub async fn sample_stats_once(state: &AppState) -> anyhow::Result<()> {
    let stats = collect_stats(state).await?;
    state.stats_cache.store(stats).await;
    Ok(())
}

pub async fn run_stats_sampler(state: std::sync::Arc<AppState>) {
    loop {
        if let Err(err) = sample_stats_once(&state).await {
            tracing::warn!(%err, "stats sample failed");
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
}

async fn stats_snapshot(pool: &crate::db::Pool) -> sqlx::Result<StatsSnapshot> {
    sqlx::query_as(
        "SELECT
            pg_database_size(current_database())::BIGINT AS database_size_bytes,
            (SELECT COUNT(*)::BIGINT
               FROM pty_sessions
              WHERE state = 'live' AND current_session_uuid IS NOT NULL) AS live_agent_sessions,
            (SELECT COUNT(*)::BIGINT FROM events) AS event_rows,
            (SELECT COUNT(*)::BIGINT FROM claude_sessions) AS agent_sessions,
            (SELECT COUNT(*)::BIGINT FROM pty_sessions) AS pty_sessions,
            (SELECT COUNT(*)::BIGINT FROM ingester_state) AS tracked_files",
    )
    .fetch_one(pool)
    .await
}
