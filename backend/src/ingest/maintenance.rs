use anyhow::Context;

use crate::db::Pool;

const CANONICAL_BLOCKS_KEY: &str = "canonical_blocks";
// v2: codex code-mode `exec` inputs re-derived from raw JS strings into
// the canonical {command, code} shape.
// v3: the claude `Agent` tool canonicalizes to `task`; blocks that
// predate the mapping get re-derived.
const CANONICAL_BLOCKS_VERSION: i32 = 3;
const TIMELINE_PROJECTION_KEY: &str = "timeline_projection";
// v3: sessions with codex `exec` operations reprojected after their
// inputs were re-canonicalized into the {command, code} shape.
// v4: local slash-command user records (`<command-name>` etc.) no longer
// seed turns; sessions whose previews show them get reprojected.
// v5: per-turn token usage columns; sessions with no usage recorded yet
// get reprojected to fill them.
// v6: bookkeeping no longer seeds turns and Agent spawns project as
// task delegations; sessions showing orphan first turns or agent pairs
// get reprojected.
// v7: concurrent subagent turns are keyed by origin session instead of
// byte offset (offset-0 collisions clobbered one another); sessions
// with descendants get reprojected.
const TIMELINE_PROJECTION_VERSION: i32 = 7;
const USAGE_PROJECTION_KEY: &str = "usage_projection";
const USAGE_PROJECTION_VERSION: i32 = 1;

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct StartupMaintenanceStats {
    pub canonical_events_backfilled: u64,
    pub timeline_sessions_backfilled: u64,
    pub usage_sessions_backfilled: u64,
}

pub async fn run_required_startup_maintenance(
    pool: &Pool,
) -> anyhow::Result<StartupMaintenanceStats> {
    let mut stats = StartupMaintenanceStats::default();
    repair_canonical_blocks_if_behind(pool, &mut stats).await?;
    repair_timeline_projection_if_behind(pool, &mut stats).await?;
    repair_usage_projection_if_behind(pool, &mut stats).await?;
    Ok(stats)
}

/// Version check shared by the repair passes: Some(current) when the
/// stored version is behind and the repair should run; None otherwise
/// (with a warning when the database is ahead of this binary).
async fn version_behind(pool: &Pool, key: &str, target: i32) -> anyhow::Result<Option<i32>> {
    let current = projection_version(pool, key).await?;
    if current < target {
        return Ok(Some(current));
    }
    if current > target {
        tracing::warn!(
            key,
            current,
            target,
            "database has newer derived data version; skipping repair",
        );
    }
    Ok(None)
}

async fn repair_canonical_blocks_if_behind(
    pool: &Pool,
    stats: &mut StartupMaintenanceStats,
) -> anyhow::Result<()> {
    let Some(current) =
        version_behind(pool, CANONICAL_BLOCKS_KEY, CANONICAL_BLOCKS_VERSION).await?
    else {
        return Ok(());
    };
    tracing::info!(
        key = CANONICAL_BLOCKS_KEY,
        current,
        target = CANONICAL_BLOCKS_VERSION,
        "derived transcript data version behind; repairing missing canonical fields",
    );
    let job = super::jobs::start(
        pool,
        "canonical_backfill",
        "Canonical block repair",
        "items",
        None,
    )
    .await
    .ok();
    let outcome = match super::ingester::backfill_canonical_blocks(pool, job.as_ref()).await {
        Ok(outcome) => outcome,
        Err(err) => {
            if let Some(job) = &job {
                job.fail(&format!("{err:#}")).await;
            }
            return Err(err).context("backfill canonical blocks");
        }
    };
    if let Some(job) = &job {
        if outcome.failed > 0 {
            job.fail(&format!(
                "{} rows failed; retried next startup",
                outcome.failed
            ))
            .await;
        } else {
            job.complete().await;
        }
    }
    stats.canonical_events_backfilled = outcome.repaired as u64;
    if outcome.failed == 0 {
        set_projection_version(pool, CANONICAL_BLOCKS_KEY, CANONICAL_BLOCKS_VERSION).await?;
    } else {
        // Failed rows were logged individually with their error
        // chains. Holding the version back retries them next
        // startup; the repaired sessions are no-ops by then.
        tracing::warn!(
            failed = outcome.failed,
            repaired = outcome.repaired,
            "canonical repair incomplete; version gate held for retry",
        );
    }
    Ok(())
}

async fn repair_timeline_projection_if_behind(
    pool: &Pool,
    stats: &mut StartupMaintenanceStats,
) -> anyhow::Result<()> {
    let Some(current) =
        version_behind(pool, TIMELINE_PROJECTION_KEY, TIMELINE_PROJECTION_VERSION).await?
    else {
        return Ok(());
    };
    tracing::info!(
        key = TIMELINE_PROJECTION_KEY,
        current,
        target = TIMELINE_PROJECTION_VERSION,
        "derived transcript data version behind; repairing missing timeline projection rows",
    );
    let job = super::jobs::start(
        pool,
        "timeline_backfill",
        "Timeline projection rebuild",
        "sessions",
        None,
    )
    .await
    .ok();
    match super::projection::backfill_timeline_projection(pool, job.as_ref()).await {
        Ok(rebuilt) => {
            if let Some(job) = &job {
                job.complete().await;
            }
            stats.timeline_sessions_backfilled = rebuilt as u64;
        }
        Err(err) => {
            if let Some(job) = &job {
                job.fail(&format!("{err:#}")).await;
            }
            return Err(err).context("backfill timeline projection");
        }
    }
    set_projection_version(pool, TIMELINE_PROJECTION_KEY, TIMELINE_PROJECTION_VERSION).await?;
    Ok(())
}

async fn repair_usage_projection_if_behind(
    pool: &Pool,
    stats: &mut StartupMaintenanceStats,
) -> anyhow::Result<()> {
    let Some(current) =
        version_behind(pool, USAGE_PROJECTION_KEY, USAGE_PROJECTION_VERSION).await?
    else {
        return Ok(());
    };
    tracing::info!(
        key = USAGE_PROJECTION_KEY,
        current,
        target = USAGE_PROJECTION_VERSION,
        "derived usage data version behind; rebuilding token categories",
    );
    let job = super::jobs::start(
        pool,
        "usage_backfill",
        "Usage projection rebuild",
        "sessions",
        None,
    )
    .await
    .ok();
    match super::usage::rebuild_usage_projection(pool).await {
        Ok(rebuilt) => {
            if let Some(job) = &job {
                job.complete().await;
            }
            stats.usage_sessions_backfilled = rebuilt;
        }
        Err(err) => {
            if let Some(job) = &job {
                job.fail(&format!("{err:#}")).await;
            }
            return Err(err).context("rebuild usage projection");
        }
    }
    set_projection_version(pool, USAGE_PROJECTION_KEY, USAGE_PROJECTION_VERSION).await?;
    Ok(())
}

pub async fn mark_projection_versions_current(pool: &Pool) -> anyhow::Result<()> {
    set_projection_version(pool, CANONICAL_BLOCKS_KEY, CANONICAL_BLOCKS_VERSION).await?;
    set_projection_version(pool, TIMELINE_PROJECTION_KEY, TIMELINE_PROJECTION_VERSION).await?;
    set_projection_version(pool, USAGE_PROJECTION_KEY, USAGE_PROJECTION_VERSION).await?;
    Ok(())
}

async fn projection_version(pool: &Pool, name: &str) -> anyhow::Result<i32> {
    let version = sqlx::query_scalar(
        "SELECT version \
           FROM ingest_projection_versions \
          WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("load ingest projection version {name}"))?;

    Ok(version.unwrap_or(0))
}

async fn set_projection_version(pool: &Pool, name: &str, version: i32) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO ingest_projection_versions (name, version, updated_at) \
         VALUES ($1, $2, NOW()) \
         ON CONFLICT (name) DO UPDATE SET \
             version = EXCLUDED.version, \
             updated_at = NOW()",
    )
    .bind(name)
    .bind(version)
    .execute(pool)
    .await
    .with_context(|| format!("set ingest projection version {name}"))?;
    Ok(())
}
