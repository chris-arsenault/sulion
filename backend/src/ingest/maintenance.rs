use anyhow::Context;

use crate::db::Pool;

const CANONICAL_BLOCKS_KEY: &str = "canonical_blocks";
// v2: codex code-mode `exec` inputs re-derived from raw JS strings into
// the canonical {command, code} shape.
const CANONICAL_BLOCKS_VERSION: i32 = 2;
const TIMELINE_PROJECTION_KEY: &str = "timeline_projection";
// v3: sessions with codex `exec` operations reprojected after their
// inputs were re-canonicalized into the {command, code} shape.
// v4: local slash-command user records (`<command-name>` etc.) no longer
// seed turns; sessions whose previews show them get reprojected.
const TIMELINE_PROJECTION_VERSION: i32 = 4;
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

    let canonical_version = projection_version(pool, CANONICAL_BLOCKS_KEY).await?;
    if canonical_version < CANONICAL_BLOCKS_VERSION {
        tracing::info!(
            key = CANONICAL_BLOCKS_KEY,
            current = canonical_version,
            target = CANONICAL_BLOCKS_VERSION,
            "derived transcript data version behind; repairing missing canonical fields",
        );
        stats.canonical_events_backfilled = super::ingester::backfill_canonical_blocks(pool)
            .await
            .context("backfill canonical blocks")?
            as u64;
        set_projection_version(pool, CANONICAL_BLOCKS_KEY, CANONICAL_BLOCKS_VERSION).await?;
    } else if canonical_version > CANONICAL_BLOCKS_VERSION {
        tracing::warn!(
            key = CANONICAL_BLOCKS_KEY,
            current = canonical_version,
            target = CANONICAL_BLOCKS_VERSION,
            "database has newer derived transcript data version; skipping canonical repair",
        );
    }

    let timeline_version = projection_version(pool, TIMELINE_PROJECTION_KEY).await?;
    if timeline_version < TIMELINE_PROJECTION_VERSION {
        tracing::info!(
            key = TIMELINE_PROJECTION_KEY,
            current = timeline_version,
            target = TIMELINE_PROJECTION_VERSION,
            "derived transcript data version behind; repairing missing timeline projection rows",
        );
        stats.timeline_sessions_backfilled = super::projection::backfill_timeline_projection(pool)
            .await
            .context("backfill timeline projection")?
            as u64;
        set_projection_version(pool, TIMELINE_PROJECTION_KEY, TIMELINE_PROJECTION_VERSION).await?;
    } else if timeline_version > TIMELINE_PROJECTION_VERSION {
        tracing::warn!(
            key = TIMELINE_PROJECTION_KEY,
            current = timeline_version,
            target = TIMELINE_PROJECTION_VERSION,
            "database has newer derived transcript data version; skipping timeline repair",
        );
    }

    let usage_version = projection_version(pool, USAGE_PROJECTION_KEY).await?;
    if usage_version < USAGE_PROJECTION_VERSION {
        tracing::info!(
            key = USAGE_PROJECTION_KEY,
            current = usage_version,
            target = USAGE_PROJECTION_VERSION,
            "derived usage data version behind; rebuilding token categories",
        );
        stats.usage_sessions_backfilled = super::usage::rebuild_usage_projection(pool)
            .await
            .context("rebuild usage projection")?;
        set_projection_version(pool, USAGE_PROJECTION_KEY, USAGE_PROJECTION_VERSION).await?;
    } else if usage_version > USAGE_PROJECTION_VERSION {
        tracing::warn!(
            key = USAGE_PROJECTION_KEY,
            current = usage_version,
            target = USAGE_PROJECTION_VERSION,
            "database has newer derived usage data version; skipping usage repair",
        );
    }

    Ok(stats)
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
