//! Progress records for long-running background work.
//!
//! Anything that can take minutes — startup backfills, a first-deploy
//! transcript catch-up — starts a job row, advances it as it goes, and
//! closes it as completed or failed. `/api/jobs` reads these rows so a
//! busy ingester looks like a progress bar instead of a stall.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use anyhow::Context as _;
use chrono::{DateTime, Utc};

use crate::db::Pool;

/// Minimum interval between progress writes for one job. The final
/// write of a run (complete/fail) always lands.
const PROGRESS_WRITE_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// A `running` job whose last write is older than this is presented as
/// stalled: its writer is either wedged or gone without cleanup.
const STALL_AFTER_SECONDS: i64 = 120;

pub struct JobHandle {
    pool: Pool,
    id: i64,
    counted: AtomicI64,
    last_write: Mutex<Instant>,
}

/// Close any still-`running` rows for `name` as interrupted. Called by
/// each writer for its own job names before starting fresh, so a crash
/// mid-run cannot leave a phantom running job forever.
pub async fn interrupt_running(pool: &Pool, name: &str) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE ingest_jobs \
            SET status = 'interrupted', updated_at = now(), finished_at = now() \
          WHERE name = $1 AND status = 'running'",
    )
    .bind(name)
    .execute(pool)
    .await
    .with_context(|| format!("interrupt running job {name}"))?;
    Ok(())
}

pub async fn start(
    pool: &Pool,
    name: &str,
    label: &str,
    unit: &str,
    total: Option<i64>,
) -> anyhow::Result<JobHandle> {
    interrupt_running(pool, name).await?;
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO ingest_jobs (name, label, unit, progress_total) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(label)
    .bind(unit)
    .bind(total)
    .fetch_one(pool)
    .await
    .with_context(|| format!("start job {name}"))?;
    Ok(JobHandle {
        pool: pool.clone(),
        id,
        counted: AtomicI64::new(0),
        last_write: Mutex::new(Instant::now() - PROGRESS_WRITE_EVERY),
    })
}

impl JobHandle {
    /// Units processed so far by this handle.
    pub fn counted(&self) -> i64 {
        self.counted.load(Ordering::Relaxed)
    }

    /// Count one unit of work and (throttled) persist the new position.
    /// Progress failures are logged, never propagated: losing a progress
    /// write must not fail the work it describes.
    pub async fn advance(&self, detail: Option<&str>) {
        let current = self.counted.fetch_add(1, Ordering::Relaxed) + 1;
        if !self.due_for_write() {
            return;
        }
        let result = sqlx::query(
            "UPDATE ingest_jobs \
                SET progress_current = $2, detail = COALESCE($3, detail), updated_at = now() \
              WHERE id = $1",
        )
        .bind(self.id)
        .bind(current)
        .bind(detail)
        .execute(&self.pool)
        .await;
        if let Err(err) = result {
            tracing::warn!(job = self.id, %err, "job progress write failed");
        }
    }

    /// Update the target size. Multi-stage jobs learn their total late,
    /// and a cross-tick catch-up extends it as new dirty files appear.
    pub async fn set_total(&self, total: i64) {
        let result = sqlx::query(
            "UPDATE ingest_jobs SET progress_total = $2, updated_at = now() WHERE id = $1",
        )
        .bind(self.id)
        .bind(total)
        .execute(&self.pool)
        .await;
        if let Err(err) = result {
            tracing::warn!(job = self.id, %err, "job total write failed");
        }
    }

    pub async fn complete(&self) {
        self.finish("completed", None).await;
    }

    pub async fn fail(&self, error: &str) {
        self.finish("failed", Some(error)).await;
    }

    async fn finish(&self, status: &str, error: Option<&str>) {
        let result = sqlx::query(
            "UPDATE ingest_jobs \
                SET status = $2, error = $3, progress_current = $4, \
                    updated_at = now(), finished_at = now() \
              WHERE id = $1",
        )
        .bind(self.id)
        .bind(status)
        .bind(error)
        .bind(self.counted())
        .execute(&self.pool)
        .await;
        if let Err(err) = result {
            tracing::warn!(job = self.id, %err, "job finish write failed");
        }
    }

    fn due_for_write(&self) -> bool {
        let mut last = self
            .last_write
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.elapsed() < PROGRESS_WRITE_EVERY {
            return false;
        }
        *last = Instant::now();
        true
    }
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct JobView {
    pub id: i64,
    pub name: String,
    pub label: String,
    pub status: String,
    pub progress_current: i64,
    pub progress_total: Option<i64>,
    pub unit: String,
    pub detail: Option<String>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Running but silent past the stall window — writer wedged or gone.
    pub stalled: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct JobsResponse {
    pub active: Vec<JobView>,
    pub recent: Vec<JobView>,
}

pub async fn list_jobs(pool: &Pool) -> anyhow::Result<JobsResponse> {
    let active: Vec<JobView> = sqlx::query_as(
        "SELECT id, name, label, status, progress_current, progress_total, unit, \
                detail, error, started_at, updated_at, finished_at, \
                (updated_at < now() - make_interval(secs => $1)) AS stalled \
           FROM ingest_jobs \
          WHERE status = 'running' \
          ORDER BY started_at ASC",
    )
    .bind(STALL_AFTER_SECONDS as f64)
    .fetch_all(pool)
    .await
    .context("list active jobs")?;
    let recent: Vec<JobView> = sqlx::query_as(
        "SELECT id, name, label, status, progress_current, progress_total, unit, \
                detail, error, started_at, updated_at, finished_at, \
                FALSE AS stalled \
           FROM ingest_jobs \
          WHERE status <> 'running' AND finished_at > now() - interval '7 days' \
          ORDER BY finished_at DESC \
          LIMIT 20",
    )
    .fetch_all(pool)
    .await
    .context("list recent jobs")?;
    Ok(JobsResponse { active, recent })
}
