//! Purging an exported session down to its turn digest.
//!
//! Everything a person can still do with archived history (search it, read a
//! turn, see which turns touched a file, report cost and churn) is served by
//! what this leaves behind: the `timeline_turns` rows with their markdown,
//! prompt, timestamps, tokens, and a `files_json` list; one `turn_digest`
//! embedding source per turn; and two daily rollups. Everything else the
//! session owned is deleted in one transaction, after its S3 object was
//! verified and its grace period passed.

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::db::Pool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PurgeCandidate {
    pub session_uuid: Uuid,
    pub archived_at: DateTime<Utc>,
}

/// Sessions exported at least `purge_after_days` ago, unchanged since, and
/// not yet purged.
pub async fn purge_candidates(
    pool: &Pool,
    purge_after_days: i64,
) -> anyhow::Result<Vec<PurgeCandidate>> {
    let rows: Vec<PurgeCandidate> = sqlx::query_as(
        "SELECT cs.session_uuid, cs.archived_at \
           FROM claude_sessions cs \
          WHERE cs.purged_at IS NULL \
            AND cs.archived_at IS NOT NULL \
            AND cs.archived_at < NOW() - make_interval(days => $1::INT) \
            AND cs.archive_events = ( \
                SELECT COUNT(*)::BIGINT FROM events e WHERE e.session_uuid = cs.session_uuid \
            ) \
            AND NOT EXISTS ( \
                SELECT 1 FROM pty_sessions ps \
                 WHERE ps.current_session_uuid = cs.session_uuid AND ps.state = 'live' \
            ) \
          ORDER BY cs.archived_at ASC",
    )
    .bind(purge_after_days as i32)
    .fetch_all(pool)
    .await
    .context("select purge candidates")?;
    Ok(rows)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PurgeOutcome {
    pub session_uuid: Uuid,
    pub repo: Option<String>,
    pub events_deleted: u64,
    pub blocks_deleted: u64,
    pub operations_deleted: u64,
    pub touches_deleted: u64,
    pub turns_kept: i64,
    pub digest_sources: u64,
    pub usage_rollup_rows: u64,
    pub file_activity_rows: u64,
}

/// The repo a session's usage is attributed to, by the same precedence the
/// metrics query uses for live sessions: the correlated PTY, the compaction
/// lineage's PTY, a reverse PTY pointer, then the transcript project hash.
/// Frozen into the rollup at purge time, since the rows it is derived from
/// may not outlive the session.
pub async fn attributed_repo(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
) -> anyhow::Result<Option<String>> {
    let repo: Option<String> = sqlx::query_scalar(
        "WITH RECURSIVE repo_hashes AS ( \
            SELECT repo_name, regexp_replace(path, '[^A-Za-z0-9]', '-', 'g') AS project_hash \
              FROM repo_runtime_state \
         ), lineage AS ( \
            SELECT cs.session_uuid AS origin, cs.pty_session_id, cs.parent_session_uuid, 0 AS depth \
              FROM claude_sessions cs WHERE cs.session_uuid = $1 \
          UNION ALL \
            SELECT l.origin, parent.pty_session_id, parent.parent_session_uuid, l.depth + 1 \
              FROM lineage l \
              JOIN claude_sessions parent ON parent.session_uuid = l.parent_session_uuid \
             WHERE l.pty_session_id IS NULL AND l.depth < 16 \
         ), lineage_pty AS ( \
            SELECT DISTINCT ON (origin) origin, pty_session_id \
              FROM lineage WHERE pty_session_id IS NOT NULL ORDER BY origin, depth \
         ) \
         SELECT COALESCE(p_direct.repo, p_reverse.repo, hash_repo.repo_name) \
           FROM claude_sessions cs \
           LEFT JOIN lineage_pty lp ON lp.origin = cs.session_uuid \
           LEFT JOIN pty_sessions p_direct ON p_direct.id = lp.pty_session_id \
           LEFT JOIN LATERAL ( \
                SELECT pr.repo FROM pty_sessions pr \
                 WHERE pr.current_session_uuid = cs.session_uuid LIMIT 1 \
           ) p_reverse ON TRUE \
           LEFT JOIN LATERAL ( \
                SELECT r.repo_name FROM repo_hashes r \
                 WHERE cs.project_hash IS NOT NULL AND r.project_hash = cs.project_hash \
                 LIMIT 1 \
           ) hash_repo ON TRUE \
          WHERE cs.session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_optional(&mut **tx)
    .await
    .context("attribute session repo")?
    .flatten();
    Ok(repo)
}

/// Purges one session. Refuses a session that is live, unexported, changed
/// since export, or already purged; those are the invariants the loop's
/// candidate query enforces, re-checked under the row lock.
pub async fn purge_session(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<PurgeOutcome> {
    let mut tx = pool.begin().await.context("begin purge tx")?;
    let guard: Option<(Option<DateTime<Utc>>, Option<i64>, Option<DateTime<Utc>>)> =
        sqlx::query_as(
            "SELECT archived_at, archive_events, purged_at \
               FROM claude_sessions WHERE session_uuid = $1 FOR UPDATE",
        )
        .bind(session_uuid)
        .fetch_optional(&mut *tx)
        .await?;
    let Some((archived_at, archive_events, purged_at)) = guard else {
        anyhow::bail!("session {session_uuid} does not exist");
    };
    if purged_at.is_some() {
        anyhow::bail!("session {session_uuid} is already purged");
    }
    if archived_at.is_none() {
        anyhow::bail!("session {session_uuid} has no verified export");
    }
    let event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM events WHERE session_uuid = $1")
            .bind(session_uuid)
            .fetch_one(&mut *tx)
            .await?;
    if archive_events != Some(event_count) {
        anyhow::bail!(
            "session {session_uuid} grew since its export ({event_count} events, {} archived)",
            archive_events.unwrap_or(0)
        );
    }
    let live: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
            SELECT 1 FROM pty_sessions \
             WHERE current_session_uuid = $1 AND state = 'live' \
         )",
    )
    .bind(session_uuid)
    .fetch_one(&mut *tx)
    .await?;
    if live {
        anyhow::bail!("session {session_uuid} is the current session of a live PTY");
    }

    let repo = attributed_repo(&mut tx, session_uuid).await?;
    let mut outcome = PurgeOutcome {
        session_uuid,
        repo: repo.clone(),
        ..PurgeOutcome::default()
    };
    let repo_key = repo.clone().unwrap_or_default();

    // Cost: freeze the per-session daily rows into the repo-level rollup,
    // remembering exactly what was added so a restore can take it back.
    outcome.usage_rollup_rows = sqlx::query(
        "INSERT INTO usage_rollup_contributions \
            (session_uuid, day, repo, agent, model, input_tokens, cached_input_tokens, \
             cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens) \
         SELECT session_uuid, day, $2, agent, model, input_tokens, cached_input_tokens, \
                cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens \
           FROM agent_model_usage_daily WHERE session_uuid = $1 \
         ON CONFLICT (session_uuid, day, repo, agent, model) DO UPDATE SET \
            input_tokens = EXCLUDED.input_tokens, \
            cached_input_tokens = EXCLUDED.cached_input_tokens, \
            cache_write_input_tokens = EXCLUDED.cache_write_input_tokens, \
            cache_write_1h_input_tokens = EXCLUDED.cache_write_1h_input_tokens, \
            output_tokens = EXCLUDED.output_tokens",
    )
    .bind(session_uuid)
    .bind(&repo_key)
    .execute(&mut *tx)
    .await
    .context("record usage contributions")?
    .rows_affected();
    sqlx::query(
        "INSERT INTO usage_daily_rollup \
            (day, repo, agent, model, input_tokens, cached_input_tokens, \
             cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens) \
         SELECT day, repo, agent, model, SUM(input_tokens), SUM(cached_input_tokens), \
                SUM(cache_write_input_tokens), SUM(cache_write_1h_input_tokens), SUM(output_tokens) \
           FROM usage_rollup_contributions WHERE session_uuid = $1 \
          GROUP BY day, repo, agent, model \
         ON CONFLICT (day, repo, agent, model) DO UPDATE SET \
            input_tokens = usage_daily_rollup.input_tokens + EXCLUDED.input_tokens, \
            cached_input_tokens = usage_daily_rollup.cached_input_tokens + EXCLUDED.cached_input_tokens, \
            cache_write_input_tokens = usage_daily_rollup.cache_write_input_tokens + EXCLUDED.cache_write_input_tokens, \
            cache_write_1h_input_tokens = usage_daily_rollup.cache_write_1h_input_tokens + EXCLUDED.cache_write_1h_input_tokens, \
            output_tokens = usage_daily_rollup.output_tokens + EXCLUDED.output_tokens, \
            updated_at = NOW()",
    )
    .bind(session_uuid)
    .execute(&mut *tx)
    .await
    .context("add usage rollup")?;

    // File churn: per repo, path, and day, how many of this session's turns
    // wrote or read the path.
    outcome.file_activity_rows = sqlx::query(
        "INSERT INTO file_activity_contributions (session_uuid, repo, path, day, write_turns, read_turns) \
         SELECT ft.session_uuid, ft.repo_name, ft.repo_rel_path, \
                (tt.end_timestamp AT TIME ZONE 'UTC')::DATE, \
                COUNT(DISTINCT ft.turn_id) FILTER (WHERE ft.is_write)::BIGINT, \
                COUNT(DISTINCT ft.turn_id) FILTER (WHERE NOT ft.is_write)::BIGINT \
           FROM timeline_file_touches ft \
           JOIN timeline_turns tt ON tt.session_uuid = ft.session_uuid AND tt.turn_id = ft.turn_id \
          WHERE ft.session_uuid = $1 \
          GROUP BY ft.session_uuid, ft.repo_name, ft.repo_rel_path, (tt.end_timestamp AT TIME ZONE 'UTC')::DATE \
         ON CONFLICT (session_uuid, repo, path, day) DO UPDATE SET \
            write_turns = EXCLUDED.write_turns, read_turns = EXCLUDED.read_turns",
    )
    .bind(session_uuid)
    .execute(&mut *tx)
    .await
    .context("record file activity contributions")?
    .rows_affected();
    sqlx::query(
        "INSERT INTO file_activity_daily (repo, path, day, write_turns, read_turns) \
         SELECT repo, path, day, write_turns, read_turns \
           FROM file_activity_contributions WHERE session_uuid = $1 \
         ON CONFLICT (repo, path, day) DO UPDATE SET \
            write_turns = file_activity_daily.write_turns + EXCLUDED.write_turns, \
            read_turns = file_activity_daily.read_turns + EXCLUDED.read_turns, \
            updated_at = NOW()",
    )
    .bind(session_uuid)
    .execute(&mut *tx)
    .await
    .context("add file activity rollup")?;

    // The digest keeps which files each turn touched, since the touch rows
    // are about to go and file-history still has to answer for these turns.
    sqlx::query(
        "UPDATE timeline_turns tt \
            SET files_json = COALESCE(( \
                SELECT jsonb_agg(DISTINCT jsonb_build_object( \
                           'repo', ft.repo_name, 'path', ft.repo_rel_path, \
                           'kind', ft.touch_kind, 'write', ft.is_write)) \
                  FROM timeline_file_touches ft \
                 WHERE ft.session_uuid = tt.session_uuid AND ft.turn_id = tt.turn_id \
            ), '[]'::jsonb), \
                chunks_json = '[]'::jsonb \
          WHERE tt.session_uuid = $1",
    )
    .bind(session_uuid)
    .execute(&mut *tx)
    .await
    .context("fold file touches into the turn digest")?;
    outcome.turns_kept =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM timeline_turns WHERE session_uuid = $1")
            .bind(session_uuid)
            .fetch_one(&mut *tx)
            .await?;

    // Semantic search over the digest: one source per turn with markdown,
    // replacing every block- and operation-level source the session had.
    sqlx::query("DELETE FROM retrieval_embeddings WHERE session_uuid = $1")
        .bind(session_uuid)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM retrieval_embedding_sources WHERE session_uuid = $1")
        .bind(session_uuid)
        .execute(&mut *tx)
        .await?;
    outcome.digest_sources = sqlx::query(
        "INSERT INTO retrieval_embedding_sources \
            (source_family, source_kind, source_key, session_uuid, turn_id, repo_name, \
             content_hash, index_status, dirty_at) \
         SELECT 'turn_digest', 'turn_digest', \
                'turn:' || session_uuid::TEXT || ':' || turn_id::TEXT, \
                session_uuid, turn_id, $2, \
                encode(sha256(convert_to(markdown, 'UTF8')), 'hex'), 'pending', NOW() \
           FROM timeline_turns \
          WHERE session_uuid = $1 AND length(trim(markdown)) > 0",
    )
    .bind(session_uuid)
    .bind(repo.as_deref())
    .execute(&mut *tx)
    .await
    .context("enqueue turn digest sources")?
    .rows_affected();

    for table in [
        "agent_usage_responses",
        "agent_model_usage_daily",
        "agent_usage_daily",
        "agent_session_usage",
        "agent_model_switches",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE session_uuid = $1"))
            .bind(session_uuid)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("purge {table}"))?;
    }
    outcome.touches_deleted =
        sqlx::query("DELETE FROM timeline_file_touches WHERE session_uuid = $1")
            .bind(session_uuid)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    sqlx::query("DELETE FROM timeline_activity_signals WHERE session_uuid = $1")
        .bind(session_uuid)
        .execute(&mut *tx)
        .await?;
    outcome.operations_deleted =
        sqlx::query("DELETE FROM timeline_operations WHERE session_uuid = $1")
            .bind(session_uuid)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    outcome.blocks_deleted = sqlx::query("DELETE FROM event_blocks WHERE session_uuid = $1")
        .bind(session_uuid)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    outcome.events_deleted = sqlx::query("DELETE FROM events WHERE session_uuid = $1")
        .bind(session_uuid)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    sqlx::query("UPDATE claude_sessions SET purged_at = NOW() WHERE session_uuid = $1")
        .bind(session_uuid)
        .execute(&mut *tx)
        .await?;
    tx.commit().await.context("commit purge tx")?;
    Ok(outcome)
}

/// Age-based pruning of operational rows the cycle would otherwise let
/// grow without bound.
pub async fn prune_operational_rows(pool: &Pool) -> anyhow::Result<(u64, u64)> {
    let backfills = sqlx::query(
        "DELETE FROM retrieval_embedding_backfills \
          WHERE status IN ('complete', 'failed', 'cancelled') \
            AND COALESCE(finished_at, updated_at) < NOW() - INTERVAL '30 days'",
    )
    .execute(pool)
    .await?
    .rows_affected();
    let jobs = sqlx::query(
        "DELETE FROM ingest_jobs \
          WHERE finished_at IS NOT NULL AND finished_at < NOW() - INTERVAL '90 days'",
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok((backfills, jobs))
}
