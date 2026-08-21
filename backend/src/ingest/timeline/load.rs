use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::db::Pool;
use crate::ingest::canonical::{Block, BlockKind, OperationCategory};

use super::{ResolvedSession, SessionEventFilter, SessionLookup, StoredEvent};

pub async fn resolve_session_target(
    pool: &Pool,
    pty_id: Uuid,
    explicit_session: Option<Uuid>,
) -> Result<SessionLookup, sqlx::Error> {
    let session_uuid = match explicit_session {
        Some(uuid) => Some(uuid),
        None => {
            let row: Option<(Option<Uuid>,)> =
                sqlx::query_as("SELECT current_session_uuid FROM pty_sessions WHERE id = $1")
                    .bind(pty_id)
                    .fetch_optional(pool)
                    .await?;
            match row {
                Some((Some(uuid),)) => Some(uuid),
                Some((None,)) => None,
                None => return Ok(SessionLookup::MissingPty),
            }
        }
    };

    let Some(session_uuid) = session_uuid else {
        return Ok(SessionLookup::NoSession);
    };

    let session_agent: Option<(String,)> =
        sqlx::query_as("SELECT agent FROM claude_sessions WHERE session_uuid = $1")
            .bind(session_uuid)
            .fetch_optional(pool)
            .await?;

    Ok(SessionLookup::Resolved(ResolvedSession {
        session_uuid,
        session_agent: session_agent.map(|(agent,)| agent),
    }))
}

pub async fn count_session_events(pool: &Pool, session_uuid: Uuid) -> Result<i64, sqlx::Error> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM events WHERE session_uuid = $1")
        .bind(session_uuid)
        .fetch_one(pool)
        .await?;
    Ok(count)
}

pub async fn load_session_events(
    pool: &Pool,
    session_uuid: Uuid,
    filter: &SessionEventFilter,
) -> Result<Vec<StoredEvent>, sqlx::Error> {
    type HistoryRow = (
        i64,
        DateTime<Utc>,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        bool,
        bool,
        Option<String>,
        Option<Value>,
        Option<String>,
    );

    let after = filter.after.unwrap_or(-1);
    let limit = filter.limit.map(|value| value.clamp(1, 5000));

    let rows: Vec<HistoryRow> = match (&filter.kind, limit) {
        (Some(kind), Some(limit)) => {
            sqlx::query_as(
                "SELECT byte_offset, timestamp, kind, agent, speaker, content_kind, \
                        event_uuid, parent_event_uuid, related_tool_use_id, is_sidechain, is_meta, subtype, \
                        CASE WHEN agent = 'codex' THEN payload #> '{payload,info,total_token_usage}' \
                             ELSE payload #> '{message,usage}' END, \
                        payload #>> '{message,id}' \
                   FROM events \
                  WHERE session_uuid = $1 AND byte_offset > $2 AND kind = $3 \
                  ORDER BY byte_offset ASC \
                  LIMIT $4",
            )
            .bind(session_uuid)
            .bind(after)
            .bind(kind)
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
        (Some(kind), None) => {
            sqlx::query_as(
                "SELECT byte_offset, timestamp, kind, agent, speaker, content_kind, \
                        event_uuid, parent_event_uuid, related_tool_use_id, is_sidechain, is_meta, subtype, \
                        CASE WHEN agent = 'codex' THEN payload #> '{payload,info,total_token_usage}' \
                             ELSE payload #> '{message,usage}' END, \
                        payload #>> '{message,id}' \
                   FROM events \
                  WHERE session_uuid = $1 AND byte_offset > $2 AND kind = $3 \
                  ORDER BY byte_offset ASC",
            )
            .bind(session_uuid)
            .bind(after)
            .bind(kind)
            .fetch_all(pool)
            .await?
        }
        (None, Some(limit)) => {
            sqlx::query_as(
                "SELECT byte_offset, timestamp, kind, agent, speaker, content_kind, \
                        event_uuid, parent_event_uuid, related_tool_use_id, is_sidechain, is_meta, subtype, \
                        CASE WHEN agent = 'codex' THEN payload #> '{payload,info,total_token_usage}' \
                             ELSE payload #> '{message,usage}' END, \
                        payload #>> '{message,id}' \
                   FROM events \
                  WHERE session_uuid = $1 AND byte_offset > $2 \
                  ORDER BY byte_offset ASC \
                  LIMIT $3",
            )
            .bind(session_uuid)
            .bind(after)
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
        (None, None) => {
            sqlx::query_as(
                "SELECT byte_offset, timestamp, kind, agent, speaker, content_kind, \
                        event_uuid, parent_event_uuid, related_tool_use_id, is_sidechain, is_meta, subtype, \
                        CASE WHEN agent = 'codex' THEN payload #> '{payload,info,total_token_usage}' \
                             ELSE payload #> '{message,usage}' END, \
                        payload #>> '{message,id}' \
                   FROM events \
                  WHERE session_uuid = $1 AND byte_offset > $2 \
                  ORDER BY byte_offset ASC",
            )
            .bind(session_uuid)
            .bind(after)
            .fetch_all(pool)
            .await?
        }
    };

    let blocks_by_offset =
        load_event_blocks(pool, session_uuid, rows.iter().map(|row| row.0)).await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                byte_offset,
                timestamp,
                kind,
                agent,
                speaker,
                content_kind,
                event_uuid,
                parent_event_uuid,
                related_tool_use_id,
                is_sidechain,
                is_meta,
                subtype,
                usage_json,
                usage_message_id,
            )| StoredEvent {
                byte_offset,
                timestamp,
                kind,
                agent,
                speaker,
                content_kind,
                event_uuid,
                parent_event_uuid,
                related_tool_use_id,
                is_sidechain,
                is_meta,
                subtype,
                usage_json,
                usage_message_id,
                blocks: blocks_by_offset
                    .get(&byte_offset)
                    .cloned()
                    .unwrap_or_default(),
            },
        )
        .collect())
}

pub async fn load_all_session_events(
    pool: &Pool,
    session_uuid: Uuid,
) -> Result<Vec<StoredEvent>, sqlx::Error> {
    load_session_events(
        pool,
        session_uuid,
        &SessionEventFilter {
            after: None,
            limit: None,
            kind: None,
        },
    )
    .await
}

#[derive(FromRow)]
struct EventBlockRow {
    byte_offset: i64,
    ord: i32,
    kind: String,
    text: Option<String>,
    tool_id: Option<String>,
    tool_name: Option<String>,
    tool_name_canonical: Option<String>,
    operation_type: Option<String>,
    operation_category: Option<String>,
    tool_input: Option<Value>,
    tool_output: Option<Value>,
    is_error: Option<bool>,
}

async fn load_event_blocks(
    pool: &Pool,
    session_uuid: Uuid,
    offsets: impl Iterator<Item = i64>,
) -> Result<HashMap<i64, Vec<Block>>, sqlx::Error> {
    let list: Vec<i64> = offsets.collect();
    if list.is_empty() {
        return Ok(HashMap::new());
    }

    let rows: Vec<EventBlockRow> = sqlx::query_as(
        "SELECT b.byte_offset, b.ord, b.kind, b.text, b.tool_id, b.tool_name, \
                b.tool_name_canonical, \
                CASE \
                    WHEN b.kind <> 'tool_use' THEN NULL \
                    WHEN b.tool_input ? 'file_edits' \
                         AND regexp_replace(COALESCE(b.tool_name_canonical, b.tool_name, ''), '^.*\\.', '') = 'exec' \
                        THEN 'apply_patch' \
                    ELSE COALESCE(rule.operation_type, regexp_replace(COALESCE(b.tool_name_canonical, b.tool_name, ''), '^.*\\.', '')) \
                END AS operation_type, \
                CASE \
                    WHEN b.kind <> 'tool_use' THEN NULL \
                    WHEN b.tool_input ? 'file_edits' THEN 'create_content' \
                    ELSE COALESCE(rule.operation_category, 'other') \
                END AS operation_category, \
                b.tool_input, b.tool_output, b.is_error \
           FROM event_blocks b \
           LEFT JOIN LATERAL ( \
                SELECT r.operation_type, r.operation_category \
                  FROM tool_category_rules r \
                 WHERE ( \
                        r.match_kind = 'exact' \
                    AND r.pattern = regexp_replace(COALESCE(b.tool_name_canonical, b.tool_name, ''), '^.*\\.', '') \
                 ) OR ( \
                        r.match_kind = 'prefix' \
                    AND regexp_replace(COALESCE(b.tool_name_canonical, b.tool_name, ''), '^.*\\.', '') LIKE r.pattern || '%' \
                 ) \
                 ORDER BY r.precedence ASC \
                 LIMIT 1 \
           ) rule ON TRUE \
          WHERE b.session_uuid = $1 AND b.byte_offset = ANY($2) \
          ORDER BY b.byte_offset ASC, b.ord ASC",
    )
    .bind(session_uuid)
    .bind(&list)
    .fetch_all(pool)
    .await?;

    let mut out = HashMap::new();
    for row in rows {
        let kind = match row.kind.as_str() {
            "text" => BlockKind::Text,
            "thinking" => BlockKind::Thinking,
            "tool_use" => BlockKind::ToolUse,
            "tool_result" => BlockKind::ToolResult,
            _ => BlockKind::Unknown,
        };
        out.entry(row.byte_offset)
            .or_insert_with(Vec::new)
            .push(Block {
                ord: row.ord,
                kind,
                text: row.text,
                tool_id: row.tool_id,
                tool_name: row.tool_name,
                tool_name_canonical: row.tool_name_canonical,
                operation_type: row.operation_type,
                operation_category: row
                    .operation_category
                    .as_deref()
                    .and_then(OperationCategory::parse),
                tool_input: row.tool_input,
                tool_output: row.tool_output,
                is_error: row.is_error,
                raw: None,
            });
    }
    Ok(out)
}
