use crate::db::Pool;
use uuid::Uuid;

/// Admin-triggered reindex: rebuild derived transcript tables from the
/// existing `events.payload` rows. This is deliberately not raw JSONL
/// reingestion: `events` and `ingester_state` are preserved so a missing
/// or rotated JSONL file cannot destroy the database copy of transcript
/// history.
///
/// Raw transcript replay is a different operation and should be exposed
/// separately if needed.
///
/// Tables rebuilt:
///   - `event_blocks` from `events.payload`.
///   - `timeline_turns` and the timeline projection child tables from
///     canonical event rows.
///
/// Tables preserved: `events`, `ingester_state`, `pty_sessions`,
/// `repos`, `tool_category_rules`, and `claude_sessions`.
pub async fn rebuild_ingest_derivatives(pool: &Pool) -> anyhow::Result<ReindexStats> {
    let sessions: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT session_uuid \
           FROM events \
          ORDER BY session_uuid ASC",
    )
    .fetch_all(pool)
    .await?;
    let events_preserved: i64 = sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM events")
        .fetch_one(pool)
        .await?;

    let mut tx = pool.begin().await?;

    // `timeline_turns` cascades to timeline projection child tables.
    sqlx::query("DELETE FROM timeline_turns")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM event_blocks")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE events \
            SET speaker = NULL, \
                content_kind = NULL, \
                event_uuid = NULL, \
                parent_event_uuid = NULL, \
                related_tool_use_id = NULL, \
                is_sidechain = FALSE, \
                is_meta = FALSE, \
                subtype = NULL, \
                search_text = ''",
    )
    .execute(&mut *tx)
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

    tx.commit().await?;

    let canonical_events_rebuilt = super::ingester::backfill_canonical_blocks(pool).await?;
    let mut timeline_sessions_rebuilt = 0u64;
    for (session_uuid,) in &sessions {
        super::projection::rebuild_session_projection(pool, *session_uuid).await?;
        timeline_sessions_rebuilt += 1;
    }
    super::maintenance::mark_projection_versions_current(pool).await?;

    Ok(ReindexStats {
        sessions_rebuilt: sessions.len() as u64,
        events_preserved: events_preserved.max(0) as u64,
        canonical_events_rebuilt: canonical_events_rebuilt as u64,
        timeline_sessions_rebuilt,
    })
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct ReindexStats {
    pub sessions_rebuilt: u64,
    pub events_preserved: u64,
    pub canonical_events_rebuilt: u64,
    pub timeline_sessions_rebuilt: u64,
}
