//! On-demand archive work, requested from the CLI or the admin API and
//! picked up by the loop in the control process. Rows are the bus: the CLI
//! runs on the node over the correlate socket and shares only the database
//! with control.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::db::Pool;

/// Which archived sessions a restore covers.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreScope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_uuid: Option<Uuid>,
    /// `YYYY-MM` of the session's first event, the month in its object key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default)]
    pub all: bool,
    /// Purge the session again once its replay is verified. Defaults on for
    /// `all`, so a whole-history re-index never grows the database by more
    /// than one session at a time.
    #[serde(default)]
    pub purge_after: bool,
}

impl RestoreScope {
    pub fn is_empty(&self) -> bool {
        self.session_uuid.is_none() && self.month.is_none() && self.repo.is_none() && !self.all
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveRequest {
    pub id: i64,
    pub kind: String,
    pub scope: Value,
    pub status: String,
    pub requested_by: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub result: Option<Value>,
    pub job_id: Option<i64>,
}

pub async fn enqueue_run(
    pool: &Pool,
    dry_run: bool,
    requested_by: Option<&str>,
) -> anyhow::Result<ArchiveRequest> {
    insert(
        pool,
        "run",
        serde_json::json!({ "dry_run": dry_run }),
        requested_by,
    )
    .await
}

pub async fn enqueue_verify(
    pool: &Pool,
    deep: bool,
    requested_by: Option<&str>,
) -> anyhow::Result<ArchiveRequest> {
    insert(
        pool,
        "verify",
        serde_json::json!({ "deep": deep }),
        requested_by,
    )
    .await
}

pub async fn enqueue_restore(
    pool: &Pool,
    scope: &RestoreScope,
    requested_by: Option<&str>,
) -> anyhow::Result<ArchiveRequest> {
    if scope.is_empty() {
        anyhow::bail!("restore needs a session, month, repo, or --all");
    }
    insert(pool, "restore", serde_json::to_value(scope)?, requested_by).await
}

/// Queues a restore for one session unless one is already waiting or
/// running for it. The ingester calls this when a purged session's
/// transcript grows again.
pub async fn enqueue_restore_if_absent(
    pool: &Pool,
    session_uuid: Uuid,
    requested_by: &str,
) -> anyhow::Result<bool> {
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM archive_requests \
          WHERE kind = 'restore' AND status IN ('pending', 'running') \
            AND scope ->> 'session_uuid' = $1 \
          LIMIT 1",
    )
    .bind(session_uuid.to_string())
    .fetch_optional(pool)
    .await?;
    if existing.is_some() {
        return Ok(false);
    }
    let scope = RestoreScope {
        session_uuid: Some(session_uuid),
        ..RestoreScope::default()
    };
    enqueue_restore(pool, &scope, Some(requested_by)).await?;
    Ok(true)
}

async fn insert(
    pool: &Pool,
    kind: &str,
    scope: Value,
    requested_by: Option<&str>,
) -> anyhow::Result<ArchiveRequest> {
    let row = sqlx::query(
        "INSERT INTO archive_requests (kind, scope, requested_by) \
         VALUES ($1, $2, $3) \
         RETURNING id, kind, scope, status, requested_by, requested_at, started_at, \
                   finished_at, error, result_json, job_id",
    )
    .bind(kind)
    .bind(scope)
    .bind(requested_by)
    .fetch_one(pool)
    .await?;
    Ok(from_row(&row))
}

pub async fn next_pending(pool: &Pool) -> anyhow::Result<Option<ArchiveRequest>> {
    let row = sqlx::query(
        "UPDATE archive_requests \
            SET status = 'running', started_at = NOW() \
          WHERE id = ( \
                SELECT id FROM archive_requests \
                 WHERE status = 'pending' \
                 ORDER BY requested_at ASC, id ASC \
                 LIMIT 1 \
                 FOR UPDATE SKIP LOCKED \
          ) \
          RETURNING id, kind, scope, status, requested_by, requested_at, started_at, \
                    finished_at, error, result_json, job_id",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(from_row))
}

pub async fn attach_job(pool: &Pool, id: i64, job_id: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE archive_requests SET job_id = $2 WHERE id = $1")
        .bind(id)
        .bind(job_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn finish(pool: &Pool, id: i64, result: Result<Value, String>) -> anyhow::Result<()> {
    let (status, error, result_json) = match result {
        Ok(value) => ("completed", None, Some(value)),
        Err(message) => ("failed", Some(message), None),
    };
    sqlx::query(
        "UPDATE archive_requests \
            SET status = $2, error = $3, result_json = $4, finished_at = NOW() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(error)
    .bind(result_json)
    .execute(pool)
    .await?;
    Ok(())
}

/// Rows a crashed loop left `running`, closed as failed so they do not look
/// in flight forever. Called once at loop start.
pub async fn fail_stale_running(pool: &Pool) -> anyhow::Result<u64> {
    let result = sqlx::query(
        "UPDATE archive_requests \
            SET status = 'failed', error = 'interrupted by control restart', finished_at = NOW() \
          WHERE status = 'running'",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn recent(pool: &Pool, limit: i64) -> anyhow::Result<Vec<ArchiveRequest>> {
    let rows = sqlx::query(
        "SELECT id, kind, scope, status, requested_by, requested_at, started_at, \
                finished_at, error, result_json, job_id \
           FROM archive_requests \
          ORDER BY requested_at DESC, id DESC \
          LIMIT $1",
    )
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(from_row).collect())
}

fn from_row(row: &sqlx::postgres::PgRow) -> ArchiveRequest {
    ArchiveRequest {
        id: row.try_get("id").unwrap_or_default(),
        kind: row.try_get("kind").unwrap_or_default(),
        scope: row
            .try_get("scope")
            .unwrap_or_else(|_| Value::Object(Default::default())),
        status: row.try_get("status").unwrap_or_default(),
        requested_by: row.try_get("requested_by").ok().flatten(),
        requested_at: row.try_get("requested_at").unwrap_or_else(|_| Utc::now()),
        started_at: row.try_get("started_at").ok().flatten(),
        finished_at: row.try_get("finished_at").ok().flatten(),
        error: row.try_get("error").ok().flatten(),
        result: row.try_get("result_json").ok().flatten(),
        job_id: row.try_get("job_id").ok().flatten(),
    }
}
