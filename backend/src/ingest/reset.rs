use crate::db::Pool;

/// Admin-triggered reindex: wipe every transcript row the ingester
/// produces, plus the per-file commit offsets. Terminal correlation
/// metadata is intentionally preserved: `claude_sessions.pty_session_id`
/// is populated by the PTY hook path, not by JSONL replay, so deleting
/// correlated session rows would orphan those sessions from repo
/// timelines.
///
/// The next ambient ingester tick (driven by the backend's long-
/// running ingester task on its normal poll cadence) walks every
/// JSONL from byte 0 and repopulates the tables — same code path as
/// startup. We deliberately do not synchronously drive a tick here;
/// "how many events came back" is the ingester's job to report via
/// `/api/stats`, not the admin endpoint's.
///
/// Tables cleared:
///   - `events` — cascades via ON DELETE CASCADE to `event_blocks`.
///   - `timeline_turns` — cascades via ON DELETE CASCADE to the
///     timeline projection child tables.
///   - `ingester_state` — per-session commit offsets.
///
/// Tables preserved: `pty_sessions`, `repos`, `tool_category_rules`,
/// and correlated `claude_sessions` rows. Uncorrelated `claude_sessions`
/// rows are removed because the ingester can recreate them from JSONL.
pub async fn reset_ingest_state(pool: &Pool) -> anyhow::Result<ResetStats> {
    let mut tx = pool.begin().await?;
    let sessions_cleared: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT session_uuid)::BIGINT \
           FROM ( \
             SELECT session_uuid FROM events \
             UNION \
             SELECT session_uuid FROM timeline_turns \
             UNION \
             SELECT session_uuid FROM ingester_state \
           ) transcript_sessions",
    )
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM timeline_turns")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM events").execute(&mut *tx).await?;

    let offsets_cleared: i64 = sqlx::query_scalar(
        "WITH d AS (DELETE FROM ingester_state RETURNING 1) SELECT COUNT(*) FROM d",
    )
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "UPDATE claude_sessions cs \
            SET pty_session_id = ps.id \
           FROM pty_sessions ps \
          WHERE cs.session_uuid = ps.current_session_uuid \
            AND cs.pty_session_id IS NULL",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM claude_sessions WHERE pty_session_id IS NULL")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE claude_sessions \
            SET parent_session_uuid = NULL, \
                project_hash = NULL",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(ResetStats {
        sessions_cleared: sessions_cleared.max(0) as u64,
        offsets_cleared: offsets_cleared.max(0) as u64,
    })
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct ResetStats {
    pub sessions_cleared: u64,
    pub offsets_cleared: u64,
}
