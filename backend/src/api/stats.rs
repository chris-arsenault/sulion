//! `/api/stats` — lightweight resource snapshot for in-product visibility
//! (ticket #27). Answers "is this deploy sized correctly?" without
//! ssh-ing to the host. Not a replacement for Grafana; no history,
//! no alerting, no per-session attribution.
//!
//! Process memory/CPU comes from `sysinfo`; database size plus a few
//! inventory counts come from lightweight Postgres queries. The handler
//! is intentionally cheap — it's polled every ~10s by the UI.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
use tokio::sync::Mutex;

use crate::AppState;

#[derive(Serialize)]
pub struct StatsResponse {
    pub uptime_seconds: u64,
    pub process: ProcessStats,
    pub pty: PtyStats,
    pub db: DbStats,
    pub inventory: InventoryStats,
}

#[derive(Serialize)]
pub struct ProcessStats {
    pub memory_rss_bytes: u64,
    pub cpu_percent: f32,
    /// Cgroup v2 memory ceiling when readable (`/sys/fs/cgroup/memory.max`).
    /// Null on hosts without cgroups or when the value is "max" (no limit).
    pub memory_limit_bytes: Option<u64>,
}

#[derive(Serialize)]
pub struct PtyStats {
    /// Live PTYs currently tracked by the backend process.
    pub live_sessions: usize,
    /// Live PTYs with a correlated current transcript session.
    pub live_agent_sessions: i64,
}

#[derive(Serialize)]
pub struct DbStats {
    pub database_size_bytes: i64,
}

#[derive(Serialize)]
pub struct InventoryStats {
    pub event_rows: i64,
    pub agent_sessions: i64,
    pub pty_sessions: i64,
    pub tracked_files: i64,
    pub files_seen_since_boot: u64,
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

    async fn sample(&self) -> ProcessStats {
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

pub async fn stats_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<StatsResponse>, StatusCode> {
    let uptime_seconds = state.start_time.elapsed().as_secs();
    let process = state.stats_probe.sample().await;
    let snapshot = stats_snapshot(&state.pool).await.map_err(|err| {
        tracing::warn!(%err, "db stats query failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let pty = PtyStats {
        live_sessions: state.pty.live_count().await,
        live_agent_sessions: snapshot.live_agent_sessions,
    };
    let db = DbStats {
        database_size_bytes: snapshot.database_size_bytes,
    };
    let inventory = InventoryStats {
        event_rows: snapshot.event_rows,
        agent_sessions: snapshot.agent_sessions,
        pty_sessions: snapshot.pty_sessions,
        tracked_files: snapshot.tracked_files,
        files_seen_since_boot: state.ingester.files_seen_total(),
        events_inserted_since_boot: state.ingester.events_inserted_total(),
        parse_errors_since_boot: state.ingester.parse_errors_total(),
    };
    Ok(Json(StatsResponse {
        uptime_seconds,
        process,
        pty,
        db,
        inventory,
    }))
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
