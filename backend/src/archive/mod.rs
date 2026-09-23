//! Transcript archive: export idle sessions to object storage, dump the
//! durable tables, purge exported sessions down to their turn digest, and
//! restore any of them on request.
//!
//! Runs as one background task in the control process, next to the
//! submitted-prompts reconciler. The database is already the only complete
//! copy of transcript history (Claude Code deletes its own files after 30
//! days), so `events.payload` is the archive source and no transcript file
//! is read or written here. Design and table plan:
//! docs/plans/transcript-archive-and-purge.md.

pub mod dump;
pub mod export;
pub mod purge;
pub mod requests;
pub mod restore;
pub mod store;

use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::db::Pool;
use crate::ingest::jobs;

pub use requests::{ArchiveRequest, RestoreScope};
pub use store::ObjectStore;

/// How often the loop looks for on-demand requests and a due cycle.
const POLL_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct ArchiveConfig {
    pub store: ObjectStore,
    pub db_url: String,
    pub min_idle_days: i64,
    pub purge_after_days: i64,
    pub interval_days: i64,
    /// Whether a cycle starts with the durable `pg_dump`. Always on in
    /// production; tests that exercise export and purge without a matching
    /// `pg_dump` client turn it off.
    pub dump_enabled: bool,
}

impl ArchiveConfig {
    /// `None` when no store is configured, which disables the loop entirely.
    pub async fn from_env(db_url: &str) -> Option<Self> {
        let store = ObjectStore::from_env().await?;
        Some(Self {
            store,
            db_url: db_url.to_string(),
            min_idle_days: env_days("SULION_ARCHIVE_MIN_IDLE_DAYS", 30),
            purge_after_days: env_days("SULION_ARCHIVE_PURGE_AFTER_DAYS", 90),
            interval_days: env_days("SULION_ARCHIVE_INTERVAL_DAYS", 30),
            dump_enabled: true,
        })
    }
}

fn env_days(key: &str, default: i64) -> i64 {
    crate::config::env_optional(key)
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(default)
}

/// Writes which store this loop serves, so `sulion archive status` on the
/// node can report it without seeing the control plane's environment.
pub async fn record_store(pool: &Pool, config: &ArchiveConfig) -> anyhow::Result<()> {
    sqlx::query("UPDATE archive_state SET store = $1, loop_started_at = NOW() WHERE id = 1")
        .bind(config.store.describe())
        .execute(pool)
        .await?;
    Ok(())
}

/// The operator's gate on deletion. Off by default: a cycle exports and
/// dumps but purges nothing until `sulion archive purge-gate on` after the
/// first backup has been verified.
pub async fn purge_enabled(pool: &Pool) -> anyhow::Result<bool> {
    let enabled: Option<bool> =
        sqlx::query_scalar("SELECT purge_enabled FROM archive_state WHERE id = 1")
            .fetch_optional(pool)
            .await?;
    Ok(enabled.unwrap_or(false))
}

pub async fn set_purge_enabled(pool: &Pool, enabled: bool) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE archive_state \
            SET purge_enabled = $1, \
                purge_enabled_at = CASE WHEN $1 THEN NOW() ELSE purge_enabled_at END \
          WHERE id = 1",
    )
    .bind(enabled)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CycleOutcome {
    pub dry_run: bool,
    /// False means the purge phase was skipped because the gate is closed.
    pub purge_enabled: bool,
    pub dump_key: Option<String>,
    pub dump_bytes: i64,
    pub exported: usize,
    pub export_failures: usize,
    pub exported_bytes: i64,
    pub purged: usize,
    pub purge_failures: usize,
    pub events_deleted: u64,
    pub backfills_pruned: u64,
    pub jobs_pruned: u64,
    pub eligible_for_export: usize,
    pub eligible_for_purge: usize,
}

/// The background loop: closes requests a previous process left running,
/// then alternates between draining on-demand requests and running the
/// monthly cycle when it is due.
pub async fn run_loop(pool: Pool, config: ArchiveConfig) {
    tracing::info!(
        store = %config.store.describe(),
        min_idle_days = config.min_idle_days,
        purge_after_days = config.purge_after_days,
        interval_days = config.interval_days,
        "archive loop starting",
    );
    if let Err(err) = record_store(&pool, &config).await {
        tracing::warn!(%err, "could not record the archive store");
    }
    match requests::fail_stale_running(&pool).await {
        Ok(0) => {}
        Ok(count) => tracing::warn!(count, "archive requests interrupted by restart"),
        Err(err) => tracing::warn!(%err, "could not close stale archive requests"),
    }
    loop {
        match requests::next_pending(&pool).await {
            Ok(Some(request)) => {
                handle_request(&pool, &config, request).await;
                continue;
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(%err, "archive request poll failed"),
        }
        match cycle_due(&pool, &config).await {
            Ok(true) => {
                if let Err(err) = run_cycle(&pool, &config, false).await {
                    tracing::warn!(error = format!("{err:#}"), "archive cycle failed");
                }
            }
            Ok(false) => {}
            Err(err) => tracing::warn!(%err, "archive schedule check failed"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// How long a failed or interrupted cycle holds the next attempt off, so a
/// missing bucket or a broken `pg_dump` does not retry on every poll.
const FAILED_CYCLE_BACKOFF: chrono::Duration = chrono::Duration::hours(1);

#[derive(Debug, Default, sqlx::FromRow)]
struct CycleClock {
    last_cycle_started_at: Option<DateTime<Utc>>,
    last_cycle_completed_at: Option<DateTime<Utc>>,
}

async fn cycle_due(pool: &Pool, config: &ArchiveConfig) -> anyhow::Result<bool> {
    let clock: CycleClock = sqlx::query_as(
        "SELECT last_cycle_started_at, last_cycle_completed_at FROM archive_state WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?
    .unwrap_or_default();
    let now = Utc::now();
    let interval = chrono::Duration::days(config.interval_days);
    let completed_recently = clock
        .last_cycle_completed_at
        .is_some_and(|at| now - at < interval);
    // A start with no completion after it is a cycle that failed or was cut
    // off; give it the backoff before trying again.
    let failed_recently = clock.last_cycle_started_at.is_some_and(|started| {
        clock
            .last_cycle_completed_at
            .is_none_or(|done| done < started)
            && now - started < FAILED_CYCLE_BACKOFF
    });
    Ok(!completed_recently && !failed_recently)
}

async fn handle_request(pool: &Pool, config: &ArchiveConfig, request: ArchiveRequest) {
    let result = match request.kind.as_str() {
        "run" => {
            let dry_run = request
                .scope
                .get("dry_run")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            run_cycle(pool, config, dry_run)
                .await
                .and_then(|outcome| serde_json::to_value(outcome).map_err(Into::into))
        }
        "restore" => match serde_json::from_value::<RestoreScope>(request.scope.clone()) {
            Ok(scope) => run_restore(pool, config, &scope, Some(request.id))
                .await
                .and_then(|outcome| serde_json::to_value(outcome).map_err(Into::into)),
            Err(err) => Err(anyhow::anyhow!("invalid restore scope: {err}")),
        },
        "verify" => {
            let deep = request
                .scope
                .get("deep")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            run_verify(pool, config, deep, Some(request.id))
                .await
                .and_then(|outcome| serde_json::to_value(outcome).map_err(Into::into))
        }
        other => Err(anyhow::anyhow!("unknown archive request kind {other}")),
    };
    let finish = match result {
        Ok(value) => Ok(value),
        Err(err) => {
            tracing::warn!(
                request = request.id,
                error = format!("{err:#}"),
                "archive request failed"
            );
            Err(format!("{err:#}"))
        }
    };
    if let Err(err) = requests::finish(pool, request.id, finish).await {
        tracing::warn!(request = request.id, %err, "could not record archive request result");
    }
}

/// One full cycle: durable dump, export, purge, prune. A dry run reports the
/// eligible sets and touches nothing.
pub async fn run_cycle(
    pool: &Pool,
    config: &ArchiveConfig,
    dry_run: bool,
) -> anyhow::Result<CycleOutcome> {
    let mut outcome = CycleOutcome {
        dry_run,
        purge_enabled: purge_enabled(pool).await?,
        ..CycleOutcome::default()
    };
    let export_set = export::eligible_sessions(pool, config.min_idle_days).await?;
    outcome.eligible_for_export = export_set.len();
    outcome.eligible_for_purge = purge::purge_candidates(pool, config.purge_after_days)
        .await?
        .len();
    if dry_run {
        return Ok(outcome);
    }

    let job = jobs::start(
        pool,
        "archive-cycle",
        "Archive cycle",
        "sessions",
        Some((export_set.len() + outcome.eligible_for_purge) as i64 + 1),
    )
    .await
    .ok();
    sqlx::query("UPDATE archive_state SET last_cycle_started_at = NOW() WHERE id = 1")
        .execute(pool)
        .await?;

    let result = run_cycle_phases(pool, config, export_set, &mut outcome, job.as_ref()).await;
    match (&result, job.as_ref()) {
        (Ok(()), Some(job)) => job.complete().await,
        (Err(err), Some(job)) => job.fail(&format!("{err:#}")).await,
        _ => {}
    }
    result?;
    sqlx::query("UPDATE archive_state SET last_cycle_completed_at = NOW() WHERE id = 1")
        .execute(pool)
        .await?;
    tracing::info!(
        exported = outcome.exported,
        purged = outcome.purged,
        events_deleted = outcome.events_deleted,
        dump = outcome.dump_key.as_deref().unwrap_or("-"),
        "archive cycle complete",
    );
    Ok(outcome)
}

async fn run_cycle_phases(
    pool: &Pool,
    config: &ArchiveConfig,
    export_set: Vec<export::EligibleSession>,
    outcome: &mut CycleOutcome,
    job: Option<&jobs::JobHandle>,
) -> anyhow::Result<()> {
    // Nothing is purged without a fresh durable dump in the store.
    if config.dump_enabled {
        let dump = dump::dump_durable_tables(&config.store, &config.db_url)
            .await
            .context("durable dump")?;
        sqlx::query(
            "UPDATE archive_state SET last_dump_key = $1, last_dump_at = NOW() WHERE id = 1",
        )
        .bind(&dump.key)
        .execute(pool)
        .await?;
        outcome.dump_key = Some(dump.key);
        outcome.dump_bytes = dump.bytes;
    }
    if let Some(job) = job {
        job.advance(Some("durable dump")).await;
    }

    for session in &export_set {
        match export::export_session(pool, &config.store, session).await {
            Ok(exported) => {
                outcome.exported += 1;
                outcome.exported_bytes += exported.bytes;
            }
            Err(err) => {
                outcome.export_failures += 1;
                tracing::warn!(
                    session = %session.session_uuid,
                    error = format!("{err:#}"),
                    "session export failed",
                );
            }
        }
        if let Some(job) = job {
            job.advance(Some(&format!("export {}", session.session_uuid)))
                .await;
        }
    }

    // Selected after the exports so a zero-day grace purges what this cycle
    // just exported; with the production grace the two sets never overlap.
    // Nothing is selected while the operator's gate is closed.
    let purge_set = if outcome.purge_enabled {
        purge::purge_candidates(pool, config.purge_after_days).await?
    } else {
        tracing::info!(
            "archive purge gate is closed; exported without deleting (sulion archive purge-gate on)"
        );
        Vec::new()
    };
    if let Some(job) = job {
        job.set_total(1 + export_set.len() as i64 + purge_set.len() as i64)
            .await;
    }
    for candidate in &purge_set {
        match purge::purge_session(pool, candidate.session_uuid).await {
            Ok(purged) => {
                outcome.purged += 1;
                outcome.events_deleted += purged.events_deleted;
            }
            Err(err) => {
                outcome.purge_failures += 1;
                tracing::warn!(
                    session = %candidate.session_uuid,
                    error = format!("{err:#}"),
                    "session purge failed",
                );
            }
        }
        if let Some(job) = job {
            job.advance(Some(&format!("purge {}", candidate.session_uuid)))
                .await;
        }
    }

    let (backfills, jobs_pruned) = purge::prune_operational_rows(pool).await?;
    outcome.backfills_pruned = backfills;
    outcome.jobs_pruned = jobs_pruned;
    Ok(())
}

/// Verifies every archived object with a progress job, so a deep pass over
/// thousands of objects is visible in the Jobs panel while it runs.
pub async fn run_verify(
    pool: &Pool,
    config: &ArchiveConfig,
    deep: bool,
    request_id: Option<i64>,
) -> anyhow::Result<export::VerifyOutcome> {
    let job = jobs::start(
        pool,
        "archive-verify",
        if deep {
            "Verify archive (deep)"
        } else {
            "Verify archive"
        },
        "objects",
        None,
    )
    .await
    .ok();
    if let (Some(job), Some(request_id)) = (job.as_ref(), request_id) {
        let _ = requests::attach_job(pool, request_id, job.id()).await;
    }
    let result = export::verify_archives(pool, &config.store, deep, job.as_ref()).await;
    match (&result, job.as_ref()) {
        (Ok(outcome), Some(job)) if outcome.missing + outcome.mismatched == 0 => {
            job.complete().await
        }
        (Ok(outcome), Some(job)) => {
            job.fail(&format!(
                "{} missing, {} mismatched",
                outcome.missing, outcome.mismatched
            ))
            .await
        }
        (Err(err), Some(job)) => job.fail(&format!("{err:#}")).await,
        _ => {}
    }
    result
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RestoreRunOutcome {
    pub sessions: usize,
    pub restored: usize,
    pub failures: usize,
    pub purged_again: usize,
    pub outcomes: Vec<restore::RestoreOutcome>,
    pub errors: Vec<String>,
}

/// Restores every purged session in scope, one at a time. With
/// `purge_after`, each is purged again once its replay is verified, so the
/// database holds at most one extra session's worth of rows at any moment.
pub async fn run_restore(
    pool: &Pool,
    config: &ArchiveConfig,
    scope: &RestoreScope,
    request_id: Option<i64>,
) -> anyhow::Result<RestoreRunOutcome> {
    let sessions = restore::sessions_in_scope(pool, scope).await?;
    let gate_open = purge_enabled(pool).await?;
    let purge_after = (scope.purge_after || scope.all) && gate_open;
    let mut outcome = RestoreRunOutcome {
        sessions: sessions.len(),
        ..RestoreRunOutcome::default()
    };
    if (scope.purge_after || scope.all) && !gate_open {
        outcome
            .errors
            .push("purge gate is closed; sessions were restored but not purged again".into());
    }
    if sessions.is_empty() {
        return Ok(outcome);
    }
    let job = jobs::start(
        pool,
        "archive-restore",
        "Restore archived sessions",
        "sessions",
        Some(sessions.len() as i64),
    )
    .await
    .ok();
    if let (Some(job), Some(request_id)) = (job.as_ref(), request_id) {
        let _ = requests::attach_job(pool, request_id, job.id()).await;
    }
    for session_uuid in sessions {
        let label = restore::session_label(pool, session_uuid).await;
        match restore::restore_session(pool, &config.store, session_uuid, purge_after).await {
            Ok(restored) => {
                outcome.restored += 1;
                if restored.purged_again {
                    outcome.purged_again += 1;
                }
                outcome.outcomes.push(restored);
            }
            Err(err) => {
                outcome.failures += 1;
                outcome.errors.push(format!("{label}: {err:#}"));
                tracing::warn!(session = %session_uuid, error = format!("{err:#}"), "restore failed");
            }
        }
        if let Some(job) = job.as_ref() {
            job.advance(Some(&label)).await;
        }
    }
    if let Some(job) = job.as_ref() {
        if outcome.failures == 0 {
            job.complete().await;
        } else {
            job.fail(&format!(
                "{} of {} restores failed",
                outcome.failures, outcome.sessions
            ))
            .await;
        }
    }
    Ok(outcome)
}

#[derive(Debug, Clone, Serialize)]
pub struct ArchiveStatus {
    /// A control process has started the loop against a store.
    pub configured: bool,
    pub store: Option<String>,
    pub loop_started_at: Option<DateTime<Utc>>,
    pub purge_enabled: bool,
    pub purge_enabled_at: Option<DateTime<Utc>>,
    pub last_cycle_started_at: Option<DateTime<Utc>>,
    pub last_cycle_completed_at: Option<DateTime<Utc>>,
    pub last_dump_key: Option<String>,
    pub last_dump_at: Option<DateTime<Utc>>,
    pub sessions_archived: i64,
    pub sessions_purged: i64,
    pub archived_bytes: i64,
    pub pending_requests: i64,
    pub recent_requests: Vec<ArchiveRequest>,
}

pub async fn status(pool: &Pool) -> anyhow::Result<ArchiveStatus> {
    let state = sqlx::query(
        "SELECT last_cycle_started_at, last_cycle_completed_at, last_dump_key, last_dump_at, \
                purge_enabled, purge_enabled_at, store, loop_started_at \
           FROM archive_state WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;
    let counts = sqlx::query(
        "SELECT COUNT(*) FILTER (WHERE archived_at IS NOT NULL)::BIGINT AS archived, \
                COUNT(*) FILTER (WHERE purged_at IS NOT NULL)::BIGINT AS purged, \
                COALESCE(SUM(archive_bytes) FILTER (WHERE archived_at IS NOT NULL), 0)::BIGINT AS bytes \
           FROM claude_sessions",
    )
    .fetch_one(pool)
    .await?;
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM archive_requests WHERE status IN ('pending', 'running')",
    )
    .fetch_one(pool)
    .await?;
    use sqlx::Row;
    let store: Option<String> = state
        .as_ref()
        .and_then(|row| row.try_get("store").ok().flatten());
    Ok(ArchiveStatus {
        configured: store.is_some(),
        store,
        loop_started_at: state
            .as_ref()
            .and_then(|row| row.try_get("loop_started_at").ok().flatten()),
        purge_enabled: state
            .as_ref()
            .and_then(|row| row.try_get("purge_enabled").ok())
            .unwrap_or(false),
        purge_enabled_at: state
            .as_ref()
            .and_then(|row| row.try_get("purge_enabled_at").ok().flatten()),
        last_cycle_started_at: state
            .as_ref()
            .and_then(|row| row.try_get("last_cycle_started_at").ok().flatten()),
        last_cycle_completed_at: state
            .as_ref()
            .and_then(|row| row.try_get("last_cycle_completed_at").ok().flatten()),
        last_dump_key: state
            .as_ref()
            .and_then(|row| row.try_get("last_dump_key").ok().flatten()),
        last_dump_at: state
            .as_ref()
            .and_then(|row| row.try_get("last_dump_at").ok().flatten()),
        sessions_archived: counts.try_get("archived").unwrap_or(0),
        sessions_purged: counts.try_get("purged").unwrap_or(0),
        archived_bytes: counts.try_get("bytes").unwrap_or(0),
        pending_requests: pending,
        recent_requests: requests::recent(pool, 20).await?,
    })
}
