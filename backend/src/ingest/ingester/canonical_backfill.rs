use anyhow::Context as _;

use super::*;

async fn rewrite_canonical_event(
    pool: &Pool,
    session_uuid: Uuid,
    byte_offset: i64,
    parsed: &super::super::canonical::CanonicalEvent,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE events SET speaker = $3, content_kind = $4, \
                event_uuid = $5, parent_event_uuid = $6, related_tool_use_id = $7, \
                is_sidechain = $8, is_meta = $9, subtype = $10, search_text = $11 \
         WHERE session_uuid = $1 AND byte_offset = $2",
    )
    .bind(session_uuid)
    .bind(byte_offset)
    .bind(parsed.speaker.as_str())
    .bind(parsed.content_kind.as_str())
    .bind(parsed.event_uuid.as_deref())
    .bind(parsed.parent_event_uuid.as_deref())
    .bind(parsed.related_tool_use_id.as_deref())
    .bind(parsed.is_sidechain)
    .bind(parsed.is_meta)
    .bind(parsed.subtype.as_deref())
    .bind(parsed.search_text())
    .execute(&mut *tx)
    .await?;
    insert_blocks(
        &mut tx,
        session_uuid,
        byte_offset,
        parsed.speaker,
        &parsed.blocks,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// One-shot backfill on startup: walk every event that hasn't been
/// decomposed into blocks yet and synthesise them from `payload`. Keeps
/// the frontend's "read from blocks only" invariant true even for rows
/// that pre-date this migration. This must stay partial: startup callers
/// gate it by derived-data version and it should only repair rows with
/// missing structured fields or known legacy block shapes.
/// What a canonical repair pass accomplished. `failed` > 0 means some
/// rows are still on the old shape; the caller must hold the version
/// gate back so the next startup retries them.
pub struct CanonicalBackfillOutcome {
    pub repaired: usize,
    pub failed: usize,
}

pub async fn backfill_canonical_blocks(pool: &Pool) -> anyhow::Result<CanonicalBackfillOutcome> {
    let codex_sessions: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT session_uuid \
         FROM events e \
         WHERE e.agent = 'codex' AND ( \
             e.speaker IS NULL OR \
             e.content_kind IS NULL OR \
             (e.content_kind IS DISTINCT FROM 'none' AND NOT EXISTS ( \
                 SELECT 1 FROM event_blocks b \
                  WHERE b.session_uuid = e.session_uuid \
                    AND b.byte_offset = e.byte_offset \
             )) OR \
             EXISTS ( \
                 SELECT 1 FROM event_blocks b \
                  WHERE b.session_uuid = e.session_uuid \
                    AND b.byte_offset = e.byte_offset \
                    AND b.kind = 'tool_use' \
                    AND COALESCE(b.tool_name_canonical, b.tool_name) = 'exec' \
                    AND jsonb_typeof(b.tool_input) = 'string' \
             ) \
         )",
    )
    .fetch_all(pool)
    .await?;
    let mut count = 0usize;
    let mut failures = 0usize;
    for (session_uuid,) in codex_sessions {
        // One poisoned session must not abort the whole repair: log the
        // full error chain with its location and keep going. The version
        // gate is only advanced when nothing failed, so skipped sessions
        // are retried on the next startup.
        match repair_codex_session(pool, session_uuid).await {
            Ok(repaired) => count += repaired,
            Err(err) => {
                failures += 1;
                tracing::warn!(
                    session = %session_uuid,
                    error = format!("{err:#}"),
                    "canonical backfill failed for codex session; skipping",
                );
            }
        }
    }
    let rows: Vec<(Uuid, i64, String, serde_json::Value)> = sqlx::query_as(
        "SELECT e.session_uuid, e.byte_offset, e.agent, e.payload \
         FROM events e \
         WHERE e.agent <> 'codex' AND ( \
             e.speaker IS NULL OR \
             e.content_kind IS NULL OR \
             EXISTS ( \
                 SELECT 1 FROM event_blocks b \
                  WHERE b.session_uuid = e.session_uuid \
                    AND b.byte_offset = e.byte_offset \
                    AND b.kind = 'tool_result' \
                    AND b.tool_output IS NULL \
                    AND e.payload ? 'toolUseResult' \
             ) OR \
             EXISTS ( \
                 SELECT 1 FROM event_blocks b \
                  WHERE b.session_uuid = e.session_uuid \
                    AND b.byte_offset = e.byte_offset \
                    AND b.kind = 'tool_use' \
                    AND b.tool_name_canonical IN ('edit', 'multi_edit') \
                    AND b.tool_input IS NOT NULL \
                    AND NOT (b.tool_input ? 'file_edits') \
             ) OR \
             NOT EXISTS ( \
                 SELECT 1 FROM event_blocks b \
                  WHERE b.session_uuid = e.session_uuid \
                    AND b.byte_offset = e.byte_offset \
             ) AND e.content_kind IS DISTINCT FROM 'none' \
         )",
    )
    .fetch_all(pool)
    .await?;

    for (session_uuid, byte_offset, agent, payload) in rows {
        let parsed = parse_canonical_event(&agent, &payload, session_uuid, byte_offset, None);
        match rewrite_canonical_event(pool, session_uuid, byte_offset, &parsed).await {
            Ok(()) => count += 1,
            Err(err) => {
                failures += 1;
                tracing::warn!(
                    session = %session_uuid,
                    byte_offset,
                    error = format!("{err:#}"),
                    "canonical backfill failed for event; skipping",
                );
            }
        }
    }
    Ok(CanonicalBackfillOutcome {
        repaired: count,
        failed: failures,
    })
}

/// Re-derive one codex session's canonical rows, threading the session
/// context in offset order so tool correlation stays correct.
async fn repair_codex_session(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<usize> {
    let rows: Vec<(Uuid, i64, String, serde_json::Value, bool)> = sqlx::query_as(
        "SELECT e.session_uuid, e.byte_offset, e.agent, e.payload, \
                ( \
                    e.speaker IS NULL OR \
                    e.content_kind IS NULL OR \
                    (e.content_kind IS DISTINCT FROM 'none' AND NOT EXISTS ( \
                        SELECT 1 FROM event_blocks b \
                         WHERE b.session_uuid = e.session_uuid \
                           AND b.byte_offset = e.byte_offset \
                    )) OR \
                    EXISTS ( \
                        SELECT 1 FROM event_blocks b \
                         WHERE b.session_uuid = e.session_uuid \
                           AND b.byte_offset = e.byte_offset \
                           AND b.kind = 'tool_use' \
                           AND COALESCE(b.tool_name_canonical, b.tool_name) = 'exec' \
                           AND jsonb_typeof(b.tool_input) = 'string' \
                    ) \
                ) AS needs_backfill \
         FROM events e \
         WHERE e.session_uuid = $1 \
         ORDER BY byte_offset",
    )
    .bind(session_uuid)
    .fetch_all(pool)
    .await
    .context("list session events")?;

    let mut repaired = 0usize;
    let mut ctx = CodexSessionContext::new(session_uuid);
    for (row_session_uuid, byte_offset, agent, payload, needs_backfill) in rows {
        if needs_backfill {
            let parsed =
                parse_canonical_event(&agent, &payload, row_session_uuid, byte_offset, Some(&ctx));
            rewrite_canonical_event(pool, row_session_uuid, byte_offset, &parsed)
                .await
                .with_context(|| format!("rewrite event at byte offset {byte_offset}"))?;
            if let Some(parent) = detect_codex_parent_session(&payload, row_session_uuid) {
                set_parent_session(pool, row_session_uuid, parent)
                    .await
                    .with_context(|| format!("set parent session at byte offset {byte_offset}"))?;
            }
            repaired += 1;
        }
        update_codex_context(&mut ctx, &payload, row_session_uuid);
    }
    Ok(repaired)
}

/// Scan a JSONL event payload for hints that the current session is a
/// compaction-continuation of another session. Returns the parent uuid
/// when found.
///
/// Claude Code has used different field names over time (`leafUuid`,
/// `parentSessionUuid`, snake_case variants). We accept any of them as
/// long as the referenced uuid is NOT the current session (which would
/// just be a self-reference).
pub(super) fn detect_compaction_parent(value: &Value, current: Uuid) -> Option<Uuid> {
    // If the event explicitly flags itself as a compact summary, we
    // trust whatever session hint it carries.
    let is_compact = value
        .get("isCompactSummary")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || value
            .get("type")
            .and_then(|v| v.as_str())
            .map(|s| s.eq_ignore_ascii_case("summary") || s.eq_ignore_ascii_case("compact_summary"))
            .unwrap_or(false);

    const CANDIDATES: &[&str] = &[
        "leafUuid",
        "parentSessionUuid",
        "parent_session_uuid",
        "parentSessionId",
        "parent_session_id",
        "compactedFromSessionUuid",
    ];
    for key in CANDIDATES {
        if let Some(uuid) = value
            .get(key)
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
        {
            if uuid != current && (is_compact || key != &"leafUuid") {
                return Some(uuid);
            }
            if is_compact && uuid != current {
                return Some(uuid);
            }
        }
    }
    None
}

pub(super) async fn set_parent_session(
    pool: &Pool,
    session_uuid: Uuid,
    parent: Uuid,
) -> anyhow::Result<()> {
    // Only set it once; don't let later events silently overwrite.
    sqlx::query(
        "UPDATE claude_sessions \
         SET parent_session_uuid = $2 \
         WHERE session_uuid = $1 AND parent_session_uuid IS NULL",
    )
    .bind(session_uuid)
    .bind(parent)
    .execute(pool)
    .await?;
    Ok(())
}
