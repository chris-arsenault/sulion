//! Replaying archived events through the ingester's own insert path, and
//! the guard that keeps the file poller off a session whose rows the
//! archive loop has purged.

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::db::Pool;

use super::{
    insert_event, load_codex_context, rebuild_ancestor_projections, DirtyTranscriptFile,
    InsertError, TranscriptSource,
};

pub(super) async fn session_is_purged(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<bool> {
    let purged: Option<bool> = sqlx::query_scalar(
        "SELECT purged_at IS NOT NULL FROM claude_sessions WHERE session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_optional(pool)
    .await?;
    Ok(purged.unwrap_or(false))
}

/// A purged session keeps only its turn digest; projecting appended lines
/// over that would rebuild the timeline from a handful of events. When its
/// transcript grows, ask the archive loop to restore it first and leave the
/// offset alone, so the file is revisited once the replay has brought the
/// rows back. Returns true when the file must be deferred.
pub(super) async fn defer_if_purged(
    pool: &Pool,
    file: &DirtyTranscriptFile,
) -> anyhow::Result<bool> {
    if !session_is_purged(pool, file.session_uuid).await? {
        return Ok(false);
    }
    match crate::archive::requests::enqueue_restore_if_absent(pool, file.session_uuid, "ingester")
        .await
    {
        Ok(true) => tracing::info!(
            session = %file.session_uuid,
            path = %file.path.display(),
            "transcript grew after purge; restore requested before ingest",
        ),
        Ok(false) => {}
        Err(err) => tracing::warn!(
            session = %file.session_uuid,
            %err,
            "could not request restore for a purged session",
        ),
    }
    Ok(true)
}

/// One archived event, ready to go back through the insert path.
#[derive(Debug, Clone)]
pub struct ReplayLine {
    /// The original transcript offset, so the row lands on the same
    /// idempotency key the file would produce.
    pub byte_offset: i64,
    /// The timestamp the row carried, used when the payload has none.
    pub timestamp: DateTime<Utc>,
    /// The tool-use link the row carried, which a Claude subagent line
    /// gets from a sibling meta file the archive does not hold.
    pub related_tool_use_id: Option<String>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct ReplayStats {
    pub events_inserted: u64,
    pub parse_errors: u64,
    pub turns_projected: usize,
}

/// Replays archived lines for one session through the same insert path a
/// transcript file takes, then rebuilds the session's projection in full.
/// The caller has already cleared the session's derived rows.
pub async fn replay_session_lines(
    pool: &Pool,
    session_uuid: Uuid,
    agent: &str,
    parent_session_uuid: Option<Uuid>,
    lines: Vec<ReplayLine>,
) -> anyhow::Result<ReplayStats> {
    let source = match agent {
        "codex" => TranscriptSource::Codex,
        _ => TranscriptSource::ClaudeCode,
    };
    let mut stats = ReplayStats::default();
    let mut codex_ctx = match source {
        TranscriptSource::Codex => Some(load_codex_context(pool, session_uuid).await?),
        TranscriptSource::ClaudeCode => None,
    };
    for line in &lines {
        match insert_event(
            pool,
            session_uuid,
            source,
            line.byte_offset,
            &line.payload,
            codex_ctx.as_mut(),
            line.related_tool_use_id.as_deref(),
            Some(line.timestamp),
        )
        .await
        {
            Ok(true) => stats.events_inserted += 1,
            Ok(false) => {}
            Err(InsertError::ParseFailed) => stats.parse_errors += 1,
            Err(InsertError::Db(err)) => {
                return Err(anyhow::Error::new(err).context(format!(
                    "replay {session_uuid} at offset {}",
                    line.byte_offset
                )));
            }
        }
    }
    stats.turns_projected =
        crate::ingest::projection::rebuild_session_projection(pool, session_uuid)
            .await
            .context("rebuild projection after replay")?;
    if parent_session_uuid.is_some() {
        rebuild_ancestor_projections(pool, session_uuid, source).await;
    }
    Ok(stats)
}
