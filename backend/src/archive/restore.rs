//! Bringing a purged session back from its archive object.
//!
//! The object is fetched and verified, the digest and rollup contributions
//! are taken back out, and every archived line is replayed through the same
//! insert path the ingester uses for a transcript file — so projections,
//! usage, metadata, and embedding sources rebuild exactly as they would have
//! at first ingest. No transcript file is written.

use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use crate::db::Pool;
use crate::ingest::{replay_session_lines, ReplayLine};

use super::export::read_session_lines;
use super::purge::purge_session;
use super::requests::RestoreScope;
use super::store::ObjectStore;

#[derive(Debug, Clone, Serialize)]
pub struct RestoreOutcome {
    pub session_uuid: Uuid,
    pub key: String,
    pub lines: usize,
    pub events_inserted: u64,
    pub turns: usize,
    /// Token total the rollup carried for this session before the restore,
    /// and the total the replay recomputed. A difference means parsing or
    /// dedup changed since first ingest; it is reported, not corrected.
    pub tokens_before: i64,
    pub tokens_after: i64,
    pub purged_again: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct PurgedSession {
    agent: String,
    archive_key: Option<String>,
    archive_sha256: Option<String>,
}

/// Purged sessions matching the scope, oldest archive month first, so a
/// whole-history replay walks forward in time.
pub async fn sessions_in_scope(pool: &Pool, scope: &RestoreScope) -> anyhow::Result<Vec<Uuid>> {
    if let Some(session_uuid) = scope.session_uuid {
        return Ok(vec![session_uuid]);
    }
    let month_pattern = scope.month.as_deref().map(|month| {
        let month = month.replace('-', "/");
        format!("sessions/%/{month}/%")
    });
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT cs.session_uuid \
           FROM claude_sessions cs \
           LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
           LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
          WHERE cs.purged_at IS NOT NULL \
            AND cs.archive_key IS NOT NULL \
            AND ($1::TEXT IS NULL OR cs.archive_key LIKE $1) \
            AND ($2::TEXT IS NULL OR COALESCE(ps.repo, \
                 CASE WHEN asm.cwd LIKE '/home/sulion/repos/%' THEN split_part(substr(asm.cwd, length('/home/sulion/repos/') + 1), '/', 1) \
                      WHEN asm.cwd LIKE '/home/sulion/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/sulion/workspaces/') + 1), '/', 1) \
                      ELSE NULL END) = $2) \
          ORDER BY cs.archive_key ASC",
    )
    .bind(month_pattern)
    .bind(scope.repo.as_deref())
    .fetch_all(pool)
    .await
    .context("select purged sessions in scope")?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub async fn restore_session(
    pool: &Pool,
    store: &ObjectStore,
    session_uuid: Uuid,
    purge_after: bool,
) -> anyhow::Result<RestoreOutcome> {
    let session: PurgedSession = sqlx::query_as(
        "SELECT agent, archive_key, archive_sha256 \
           FROM claude_sessions WHERE session_uuid = $1 AND purged_at IS NOT NULL",
    )
    .bind(session_uuid)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| anyhow!("session {session_uuid} is not a purged session"))?;
    let key = session
        .archive_key
        .clone()
        .ok_or_else(|| anyhow!("session {session_uuid} has no archive key"))?;
    let sha256 = session
        .archive_sha256
        .clone()
        .ok_or_else(|| anyhow!("session {session_uuid} has no archive hash"))?;

    let lines = read_session_lines(store, &key, &sha256).await?;
    if lines.is_empty() {
        anyhow::bail!("{key} holds no lines");
    }

    let tokens_before = withdraw_and_clear(pool, session_uuid).await?;

    let replay: Vec<ReplayLine> = lines
        .iter()
        .map(|line| ReplayLine {
            byte_offset: line.o,
            timestamp: line.t,
            related_tool_use_id: line.r.clone(),
            payload: serde_json::to_vec(&line.p).unwrap_or_default(),
        })
        .collect();
    let stats = replay_session_lines(pool, session_uuid, &session.agent, replay)
        .await
        .with_context(|| format!("replay {key}"))?;

    let tokens_after: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(input_tokens + cached_input_tokens + cache_write_input_tokens \
                             + cache_write_1h_input_tokens + output_tokens), 0)::BIGINT \
           FROM agent_model_usage_daily WHERE session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_one(pool)
    .await?;
    if tokens_before != tokens_after {
        tracing::warn!(
            session = %session_uuid,
            tokens_before,
            tokens_after,
            "restored usage differs from the rollup it replaced",
        );
    }

    let purged_again = if purge_after {
        purge_session(pool, session_uuid).await?;
        true
    } else {
        false
    };

    Ok(RestoreOutcome {
        session_uuid,
        key,
        lines: lines.len(),
        events_inserted: stats.events_inserted,
        turns: stats.turns_projected,
        tokens_before,
        tokens_after,
        purged_again,
    })
}

/// Takes the session's contributions back out of the rollups and clears
/// its digest and any leftover derived rows, in one transaction, before a
/// line is replayed. Returns the token total the rollup carried for it.
async fn withdraw_and_clear(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<i64> {
    let mut tx = pool.begin().await.context("begin restore tx")?;
    let tokens_before: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(input_tokens + cached_input_tokens + cache_write_input_tokens \
                             + cache_write_1h_input_tokens + output_tokens), 0)::BIGINT \
           FROM usage_rollup_contributions WHERE session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE usage_daily_rollup r \
            SET input_tokens = GREATEST(r.input_tokens - c.input_tokens, 0), \
                cached_input_tokens = GREATEST(r.cached_input_tokens - c.cached_input_tokens, 0), \
                cache_write_input_tokens = GREATEST(r.cache_write_input_tokens - c.cache_write_input_tokens, 0), \
                cache_write_1h_input_tokens = GREATEST(r.cache_write_1h_input_tokens - c.cache_write_1h_input_tokens, 0), \
                output_tokens = GREATEST(r.output_tokens - c.output_tokens, 0), \
                updated_at = NOW() \
           FROM usage_rollup_contributions c \
          WHERE c.session_uuid = $1 \
            AND r.day = c.day AND r.repo = c.repo AND r.agent = c.agent AND r.model = c.model",
    )
    .bind(session_uuid)
    .execute(&mut *tx)
    .await
    .context("subtract usage contributions")?;
    sqlx::query("DELETE FROM usage_rollup_contributions WHERE session_uuid = $1")
        .bind(session_uuid)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE file_activity_daily r \
            SET write_turns = GREATEST(r.write_turns - c.write_turns, 0), \
                read_turns = GREATEST(r.read_turns - c.read_turns, 0), \
                updated_at = NOW() \
           FROM file_activity_contributions c \
          WHERE c.session_uuid = $1 \
            AND r.repo = c.repo AND r.path = c.path AND r.day = c.day",
    )
    .bind(session_uuid)
    .execute(&mut *tx)
    .await
    .context("subtract file activity contributions")?;
    sqlx::query("DELETE FROM file_activity_contributions WHERE session_uuid = $1")
        .bind(session_uuid)
        .execute(&mut *tx)
        .await?;
    for table in [
        "retrieval_embeddings",
        "retrieval_embedding_sources",
        "timeline_turns",
        "agent_usage_responses",
        "agent_model_usage_daily",
        "agent_usage_daily",
        "agent_session_usage",
        "agent_model_switches",
        "event_blocks",
        "events",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE session_uuid = $1"))
            .bind(session_uuid)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("clear {table} before replay"))?;
    }
    // The timeline is rebuilt from the replayed events, never resumed.
    sqlx::query("UPDATE timeline_session_state SET projection_version = 0 WHERE session_uuid = $1")
        .bind(session_uuid)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE claude_sessions SET purged_at = NULL WHERE session_uuid = $1")
        .bind(session_uuid)
        .execute(&mut *tx)
        .await?;
    tx.commit().await.context("commit restore tx")?;
    Ok(tokens_before)
}

/// When the session was last touched, for the job detail line.
pub async fn session_label(pool: &Pool, session_uuid: Uuid) -> String {
    let row = sqlx::query(
        "SELECT cs.agent, ts.latest_event_at \
           FROM claude_sessions cs \
           LEFT JOIN timeline_session_state ts ON ts.session_uuid = cs.session_uuid \
          WHERE cs.session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match row {
        Some(row) => {
            let agent: String = row.try_get("agent").unwrap_or_default();
            let at: Option<DateTime<Utc>> = row.try_get("latest_event_at").ok().flatten();
            match at {
                Some(at) => format!("{agent} {session_uuid} ({})", at.format("%Y-%m-%d")),
                None => format!("{agent} {session_uuid}"),
            }
        }
        None => session_uuid.to_string(),
    }
}
