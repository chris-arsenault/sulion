//! One transcript line into `events`, its canonical blocks, usage and
//! retrieval sources, followed by the best-effort projections that read it.

use chrono::{DateTime, Utc};
use ring::digest;
use serde_json::Value;
use uuid::Uuid;

use crate::db::Pool;
use crate::ingest::activity_projection::project_from_event_best_effort;
use crate::ingest::canonical::{Block, BlockKind, CanonicalEvent, Speaker};

use super::canonical_backfill::{detect_compaction_parent, set_parent_session};
use super::codex_lineage::{
    detect_codex_parent_session, enrich_codex_lineage, update_codex_context, CodexSessionContext,
};
use super::TranscriptSource;

pub(super) enum InsertError {
    ParseFailed,
    Db(sqlx::Error),
}

/// Returns Ok(true) if an event row was inserted (i.e. not a dedupe
/// skip); Ok(false) if the line was blank/malformed and silently
/// skipped with the parse-error counter already bumped via Err at the
/// call site.
///
/// Eight parameters: the file path and the archive replay share this one
/// insert, and the two extras (`subagent_tool_use_id`, `fallback_timestamp`)
/// are the columns only one of those callers can supply.
#[allow(clippy::too_many_arguments)]
pub(super) async fn insert_event(
    pool: &Pool,
    session_uuid: Uuid,
    source: TranscriptSource,
    byte_offset: i64,
    line: &[u8],
    codex_ctx: Option<&mut CodexSessionContext>,
    subagent_tool_use_id: Option<&str>,
    fallback_timestamp: Option<DateTime<Utc>>,
) -> Result<bool, InsertError> {
    if line.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(false);
    }

    let value: Value = match serde_json::from_slice(line) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(
                %err,
                session = %session_uuid,
                byte_offset,
                "malformed JSONL line, skipping",
            );
            return Err(InsertError::ParseFailed);
        }
    };

    // Parse the line into the canonical block representation up-front
    // so the transaction below can write events + event_blocks atomically
    // (same commit). If the parser ever starts failing for a shape it
    // doesn't recognise, we log and fall back to storing the raw row
    // with no blocks — the frontend will render via `unknown` blocks or
    // the legacy payload path.
    let codex_ctx_ref = codex_ctx.as_deref();
    let mut parsed = parse_canonical_event(
        source.agent_id(),
        &value,
        session_uuid,
        byte_offset,
        codex_ctx_ref,
    );
    // Subagent transcripts link every unclaimed record back to the
    // parent's spawning tool pair so the timeline's lineage walk finds
    // the whole sidechain.
    if let Some(tool_use_id) = subagent_tool_use_id {
        if parsed.related_tool_use_id.is_none() {
            parsed.related_tool_use_id = Some(tool_use_id.to_string());
        }
    }
    let kind = stored_event_kind(source, &value, &parsed);
    let timestamp = parse_event_timestamp(&value)
        .or(fallback_timestamp)
        .unwrap_or_else(Utc::now);

    if kind == "unknown" {
        tracing::debug!(
            session = %session_uuid,
            byte_offset,
            agent = source.agent_id(),
            "event without explicit type — stored as 'unknown'",
        );
    }

    let search_text = parsed.search_text();
    let mut tx = pool.begin().await.map_err(InsertError::Db)?;

    let result = sqlx::query(
        "INSERT INTO events \
             (session_uuid, byte_offset, timestamp, kind, payload, agent, speaker, content_kind, \
              event_uuid, parent_event_uuid, related_tool_use_id, is_sidechain, is_meta, subtype, search_text) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) \
         ON CONFLICT (session_uuid, byte_offset) DO NOTHING",
    )
    .bind(session_uuid)
    .bind(byte_offset)
    .bind(timestamp)
    .bind(&kind)
    .bind(&value)
    .bind(parsed.agent)
    .bind(parsed.speaker.as_str())
    .bind(parsed.content_kind.as_str())
    .bind(parsed.event_uuid.as_deref())
    .bind(parsed.parent_event_uuid.as_deref())
    .bind(parsed.related_tool_use_id.as_deref())
    .bind(parsed.is_sidechain)
    .bind(parsed.is_meta)
    .bind(parsed.subtype.as_deref())
    .bind(&search_text)
    .execute(&mut *tx)
    .await
    .map_err(InsertError::Db)?;

    let inserted = result.rows_affected() > 0;
    if inserted {
        insert_event_derivatives(
            &mut tx,
            session_uuid,
            source,
            byte_offset,
            timestamp,
            &value,
            &parsed,
        )
        .await?;
    }

    tx.commit().await.map_err(InsertError::Db)?;

    if inserted && parsed.subtype.as_deref() != Some("inherited_history") {
        project_after_insert(pool, session_uuid, source, &value, byte_offset, timestamp).await;
    }

    if let Some(ctx) = codex_ctx {
        update_codex_context(ctx, &value, session_uuid);
    }

    link_parent_session(pool, source, &value, session_uuid).await;

    Ok(inserted)
}

/// If this event hints that the current session is a compaction or
/// subagent continuation of another, record the parent linkage. Best-effort:
/// format drift is tolerated by checking several field names.
async fn link_parent_session(
    pool: &Pool,
    source: TranscriptSource,
    value: &Value,
    session_uuid: Uuid,
) {
    let parent = match source {
        TranscriptSource::ClaudeCode => detect_compaction_parent(value, session_uuid),
        TranscriptSource::Codex => detect_codex_parent_session(value, session_uuid),
    };
    if let Some(parent) = parent {
        if let Err(err) = set_parent_session(pool, session_uuid, parent).await {
            tracing::warn!(%err, session = %session_uuid, parent = %parent, "set_parent_session failed");
        }
    }
}

/// Projections that follow a committed event row and must not fail the
/// ingest: session metadata, model-switch detection, and activity state.
/// Each is best-effort and logged on failure.
async fn project_after_insert(
    pool: &Pool,
    session_uuid: Uuid,
    source: TranscriptSource,
    value: &Value,
    byte_offset: i64,
    timestamp: DateTime<Utc>,
) {
    if let Err(err) =
        crate::ingest::metadata::upsert_from_event(pool, session_uuid, source.agent_id(), value)
            .await
    {
        tracing::warn!(
            %err,
            session = %session_uuid,
            agent = source.agent_id(),
            byte_offset,
            "agent session metadata upsert failed",
        );
    }
    // After the metadata upsert: the switch detector keeps its own
    // baselines on the same row and reads the record's model only through
    // its own extraction.
    if let Err(err) = crate::model_switches::observe_event(
        pool,
        session_uuid,
        source.agent_id(),
        value,
        byte_offset,
        timestamp,
    )
    .await
    {
        tracing::warn!(
            %err,
            session = %session_uuid,
            agent = source.agent_id(),
            byte_offset,
            "model switch observation failed",
        );
    }
    project_from_event_best_effort(pool, session_uuid, source, value, byte_offset).await;
}

async fn insert_event_derivatives(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session_uuid: Uuid,
    source: TranscriptSource,
    byte_offset: i64,
    timestamp: DateTime<Utc>,
    value: &Value,
    parsed: &CanonicalEvent,
) -> Result<(), InsertError> {
    if parsed.subtype.as_deref() == Some("inherited_history") {
        return Ok(());
    }
    if !parsed.blocks.is_empty() {
        insert_blocks(
            tx,
            session_uuid,
            byte_offset,
            parsed.speaker,
            &parsed.blocks,
        )
        .await
        .map_err(InsertError::Db)?;
    }
    crate::ingest::usage::upsert_from_event(tx, session_uuid, source, byte_offset, timestamp, value)
        .await
        .map_err(InsertError::Db)
}

pub(super) fn parse_canonical_event(
    agent: &str,
    value: &Value,
    session_uuid: Uuid,
    byte_offset: i64,
    codex_ctx: Option<&CodexSessionContext>,
) -> CanonicalEvent {
    use crate::ingest::canonical::EventParser;
    let mut parsed = match agent {
        "codex" => crate::ingest::canonical::CodexParser.parse(value),
        _ => crate::ingest::canonical::ClaudeParser.parse(value),
    };
    if agent == "codex" {
        enrich_codex_lineage(&mut parsed, value, session_uuid, byte_offset, codex_ctx);
    }
    parsed
}

fn stored_event_kind(source: TranscriptSource, value: &Value, _parsed: &CanonicalEvent) -> String {
    match source {
        TranscriptSource::ClaudeCode => value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        TranscriptSource::Codex => {
            let outer = crate::ingest::canonical::codex_record_kind(value).unwrap_or("");
            let subtype = value
                .pointer("/payload/type")
                .and_then(Value::as_str)
                .filter(|kind| !kind.is_empty())
                .unwrap_or("unknown");
            match outer {
                "response_item" | "event_msg" => subtype.to_string(),
                "" => subtype.to_string(),
                _ => outer.to_string(),
            }
        }
    }
}

fn parse_event_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    value
        .get("timestamp")
        .or_else(|| value.get("ts"))
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

pub(super) async fn insert_blocks(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session_uuid: Uuid,
    byte_offset: i64,
    speaker: Speaker,
    blocks: &[Block],
) -> sqlx::Result<()> {
    for b in blocks {
        sqlx::query(
            "INSERT INTO event_blocks \
                 (session_uuid, byte_offset, ord, kind, text, \
                  tool_id, tool_name, tool_name_canonical, tool_input, tool_output, is_error, raw) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
             ON CONFLICT (session_uuid, byte_offset, ord) DO UPDATE SET \
                 kind = EXCLUDED.kind, \
                 text = EXCLUDED.text, \
                 tool_id = EXCLUDED.tool_id, \
                 tool_name = EXCLUDED.tool_name, \
                 tool_name_canonical = EXCLUDED.tool_name_canonical, \
                 tool_input = EXCLUDED.tool_input, \
                 tool_output = EXCLUDED.tool_output, \
                 is_error = EXCLUDED.is_error, \
                 raw = EXCLUDED.raw",
        )
        .bind(session_uuid)
        .bind(byte_offset)
        .bind(b.ord)
        .bind(b.kind.as_str())
        .bind(b.text.as_deref())
        .bind(b.tool_id.as_deref())
        .bind(b.tool_name.as_deref())
        .bind(b.tool_name_canonical.as_deref())
        .bind(b.tool_input.as_ref())
        .bind(b.tool_output.as_ref())
        .bind(b.is_error)
        .bind(b.raw.as_ref())
        .execute(&mut **tx)
        .await?;
        enqueue_event_embedding_source(tx, session_uuid, byte_offset, speaker, b).await?;
    }
    Ok(())
}

async fn enqueue_event_embedding_source(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session_uuid: Uuid,
    byte_offset: i64,
    speaker: Speaker,
    block: &Block,
) -> sqlx::Result<()> {
    let source_key = format!("event:{session_uuid}:{byte_offset}:{}", block.ord);
    let Some((source_kind, text)) = event_embedding_source(speaker, block) else {
        mark_embedding_source_deleted(tx, &source_key).await?;
        return Ok(());
    };
    let text = text.trim();
    if text.is_empty() {
        mark_embedding_source_deleted(tx, &source_key).await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO retrieval_embedding_sources \
            (source_family, source_kind, source_key, session_uuid, byte_offset, block_ord, \
             content_hash, index_status, index_error, last_seen_at, dirty_at, deleted_at, updated_at) \
         VALUES ('event_block', $1, $2, $3, $4, $5, $6, 'pending', NULL, NOW(), NOW(), NULL, NOW()) \
         ON CONFLICT (source_key) DO UPDATE SET \
             source_family = 'event_block', \
             source_kind = EXCLUDED.source_kind, \
             session_uuid = EXCLUDED.session_uuid, \
             byte_offset = EXCLUDED.byte_offset, \
             block_ord = EXCLUDED.block_ord, \
             turn_id = NULL, \
             operation_ord = NULL, \
             content_hash = EXCLUDED.content_hash, \
             index_status = 'pending', \
             index_error = NULL, \
             dirty_at = NOW(), \
             deleted_at = NULL, \
             updated_at = NOW()",
    )
    .bind(source_kind)
    .bind(&source_key)
    .bind(session_uuid)
    .bind(byte_offset)
    .bind(block.ord)
    .bind(hash_text(text))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn event_embedding_source(speaker: Speaker, block: &Block) -> Option<(&'static str, &str)> {
    match (speaker, block.kind) {
        (Speaker::Assistant, BlockKind::Text) => Some(("assistant_text", block.text.as_deref()?)),
        (Speaker::User, BlockKind::Text) => Some(("user_prompt", block.text.as_deref()?)),
        (Speaker::Summary, BlockKind::Text) => Some(("summary", block.text.as_deref()?)),
        (_, BlockKind::ToolResult) if block.is_error.unwrap_or(false) => {
            Some(("tool_error", block.text.as_deref()?))
        }
        _ => None,
    }
}

async fn mark_embedding_source_deleted(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    source_key: &str,
) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM retrieval_embeddings WHERE source_key = $1")
        .bind(source_key)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE retrieval_embedding_sources \
            SET index_status = 'deleted', deleted_at = NOW(), updated_at = NOW() \
          WHERE source_key = $1",
    )
    .bind(source_key)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn hash_text(text: &str) -> String {
    let hash = digest::digest(&digest::SHA256, text.as_bytes());
    hash.as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
