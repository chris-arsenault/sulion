//! `/api/stats` — lightweight resource snapshot for in-product visibility
//! (ticket #27). Answers "is this deploy sized correctly?" without
//! ssh-ing to the host. Not a replacement for Grafana; no history,
//! no alerting, no per-session attribution.
//!
//! Process memory/CPU comes from `sysinfo`; database row counts + size
//! from a single round-trip of lightweight queries. The handler is
//! intentionally cheap — it's polled every ~10s by the UI.

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
    pub ingester: IngesterStats,
    pub pty: PtyStats,
    pub db: DbStats,
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
pub struct IngesterStats {
    pub files_seen_total: u64,
    pub events_inserted_total: u64,
    pub parse_errors_total: u64,
}

#[derive(Serialize)]
pub struct PtyStats {
    pub tracked_sessions: usize,
}

#[derive(Serialize)]
pub struct DbStats {
    pub database_size_bytes: i64,
    pub events_rowcount: i64,
    pub agent_sessions_rowcount: i64,
    pub pty_sessions_rowcount: i64,
    pub ingester_state_rowcount: i64,
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
    let ingester = IngesterStats {
        files_seen_total: state.ingester.files_seen_total(),
        events_inserted_total: state.ingester.events_inserted_total(),
        parse_errors_total: state.ingester.parse_errors_total(),
    };
    let pty = PtyStats {
        tracked_sessions: state.pty.live_count().await,
    };
    let db = db_stats(&state.pool).await.map_err(|err| {
        tracing::warn!(%err, "db stats query failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(StatsResponse {
        uptime_seconds,
        process,
        ingester,
        pty,
        db,
    }))
}

async fn db_stats(pool: &crate::db::Pool) -> sqlx::Result<DbStats> {
    // One round-trip per count — tables are small and indexed; the
    // sequential scan on `events` is the only non-trivial one, but
    // pg_class.reltuples would lie while the ingester is still
    // writing. If this becomes hot, cache the answer for a few seconds.
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(pool)
        .await?;
    let agent_sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM claude_sessions")
        .fetch_one(pool)
        .await?;
    let pty_sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pty_sessions")
        .fetch_one(pool)
        .await?;
    let ingester_state: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ingester_state")
        .fetch_one(pool)
        .await?;
    let db_size: i64 = sqlx::query_scalar("SELECT pg_database_size(current_database())::BIGINT")
        .fetch_one(pool)
        .await?;
    Ok(DbStats {
        database_size_bytes: db_size,
        events_rowcount: events,
        agent_sessions_rowcount: agent_sessions,
        pty_sessions_rowcount: pty_sessions,
        ingester_state_rowcount: ingester_state,
    })
}
