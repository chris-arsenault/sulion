use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::db::Pool;
use crate::ingest::canonical::OperationCategory;

use super::timeline::{
    build_session_projection, load_all_session_events, FileTouchContext, ProjectionFilters,
    SpeakerFacet, TimelineAssistantItem, TimelineChunk, TimelineFileTouch, TimelineResponse,
    TimelineSubagent, TimelineToolPair, TimelineToolResult, TimelineTurn,
};

const BOOKKEEPING_KINDS: &[&str] = &[
    "file-history-snapshot",
    "permission-mode",
    "last-prompt",
    "queue-operation",
    "attachment",
];

#[derive(Debug, Clone)]
pub struct RepoFileTraceTouch {
    pub pty_session_id: Option<Uuid>,
    pub session_uuid: Uuid,
    pub session_agent: Option<String>,
    pub session_label: Option<String>,
    pub session_state: Option<String>,
    pub turn_id: i64,
    pub turn_preview: String,
    pub turn_timestamp: DateTime<Utc>,
    pub operation_type: Option<String>,
    pub operation_category: Option<String>,
    /// Stable id of the tool call that did the touch, when the touch
    /// was attached to one. Lets the client jump the user straight to
    /// the specific tool row inside the turn, not just the turn head.
    pub pair_id: Option<String>,
    pub touch_kind: String,
    pub is_write: bool,
}

#[derive(FromRow)]
struct ProjectedTurnRow {
    turn_id: i64,
    preview: String,
    user_prompt_text: Option<String>,
    start_timestamp: DateTime<Utc>,
    end_timestamp: DateTime<Utc>,
    duration_ms: i64,
    event_count: i32,
    operation_count: i32,
    thinking_count: i32,
    has_errors: bool,
    markdown: String,
    chunks_json: Value,
    is_sidechain_turn: bool,
}

#[derive(FromRow)]
struct ProjectedOperationRow {
    turn_id: i64,
    operation_ord: i32,
    pair_id: String,
    name: String,
    raw_name: Option<String>,
    operation_type: Option<String>,
    operation_category: Option<String>,
    input: Option<Value>,
    result_content: Option<String>,
    result_payload: Option<Value>,
    result_is_error: bool,
    is_error: bool,
    is_pending: bool,
    subagent_json: Option<Value>,
}

#[derive(FromRow)]
struct ProjectedTouchRow {
    turn_id: i64,
    operation_ord: Option<i32>,
    repo_name: String,
    repo_rel_path: String,
    touch_kind: String,
    is_write: bool,
}

pub async fn rebuild_session_projection(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<usize> {
    let events = load_projection_source_events(pool, session_uuid)
        .await
        .context("load canonical projection events")?;
    let file_context = load_file_touch_context(pool, session_uuid).await?;
    let projected = build_session_projection(&events, file_context.as_ref());

    let mut tx = pool.begin().await.context("begin projection tx")?;
    for table in [
        "timeline_file_touches",
        "timeline_activity_signals",
        "timeline_operations",
        "timeline_turns",
    ] {
        let sql = format!("DELETE FROM {table} WHERE session_uuid = $1");
        sqlx::query(&sql)
            .bind(session_uuid)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("clear {table}"))?;
    }

    for turn in &projected {
        sqlx::query(
            "INSERT INTO timeline_turns \
                 (session_uuid, turn_id, turn_ord, is_sidechain_turn, preview, user_prompt_text, \
                  start_timestamp, end_timestamp, duration_ms, event_count, operation_count, \
                  thinking_count, has_errors, markdown, turn_json, chunks_json) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(session_uuid)
        .bind(turn.turn.id)
        .bind(turn.turn_ord)
        .bind(turn.is_sidechain_turn)
        .bind(&turn.turn.preview)
        .bind(turn.turn.user_prompt_text.as_deref())
        .bind(turn.turn.start_timestamp)
        .bind(turn.turn.end_timestamp)
        .bind(turn.turn.duration_ms)
        .bind(turn.turn.event_count as i32)
        .bind(turn.turn.operation_count as i32)
        .bind(turn.turn.thinking_count as i32)
        .bind(turn.turn.has_errors)
        .bind(&turn.turn.markdown)
        .bind(serde_json::to_value(&turn.turn).context("serialize projected turn")?)
        .bind(serde_json::to_value(&turn.turn.chunks).context("serialize projected chunks")?)
        .execute(&mut *tx)
        .await
        .context("insert timeline_turns row")?;

        for operation in &turn.operations {
            sqlx::query(
                "INSERT INTO timeline_operations \
                     (session_uuid, turn_id, operation_ord, pair_id, name, raw_name, operation_type, \
                      operation_category, input, result_content, result_payload, result_is_error, \
                      is_error, is_pending, subagent_json) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
            )
            .bind(session_uuid)
            .bind(turn.turn.id)
            .bind(operation.operation_ord)
            .bind(&operation.pair_id)
            .bind(&operation.name)
            .bind(operation.raw_name.as_deref())
            .bind(operation.operation_type.as_deref())
            .bind(operation.operation_category.map(|category| category.as_str()))
            .bind(operation.input.as_ref())
            .bind(operation.result_content.as_deref())
            .bind(operation.result_payload.as_ref())
            .bind(operation.result_is_error)
            .bind(operation.is_error)
            .bind(operation.is_pending)
            .bind(
                operation
                    .subagent
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()
                    .context("serialize projected subagent")?,
            )
            .execute(&mut *tx)
            .await
            .context("insert timeline_operations row")?;
        }

        for touch in &turn.file_touches {
            sqlx::query(
                "INSERT INTO timeline_file_touches \
                     (session_uuid, turn_id, touch_ord, operation_ord, repo_name, repo_rel_path, touch_kind, is_write) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(session_uuid)
            .bind(turn.turn.id)
            .bind(touch.touch_ord)
            .bind(touch.operation_ord)
            .bind(&touch.repo_name)
            .bind(&touch.repo_rel_path)
            .bind(&touch.touch_kind)
            .bind(touch.is_write)
            .execute(&mut *tx)
            .await
            .context("insert timeline_file_touches row")?;
        }

        for signal in &turn.activity_signals {
            sqlx::query(
                "INSERT INTO timeline_activity_signals \
                     (session_uuid, turn_id, signal_ord, signal_type, signal_value, signal_count) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(session_uuid)
            .bind(turn.turn.id)
            .bind(signal.signal_ord)
            .bind(&signal.signal_type)
            .bind(signal.signal_value.as_deref())
            .bind(signal.signal_count)
            .execute(&mut *tx)
            .await
            .context("insert timeline_activity_signals row")?;
        }
    }

    tx.commit().await.context("commit projection tx")?;
    Ok(projected.len())
}

async fn load_file_touch_context(
    pool: &Pool,
    session_uuid: Uuid,
) -> anyhow::Result<Option<FileTouchContext>> {
    #[derive(FromRow)]
    struct ContextRow {
        repo: Option<String>,
        working_dir: Option<String>,
        repo_path: Option<String>,
    }

    let row: Option<ContextRow> = sqlx::query_as(
        "SELECT ps.repo, ps.working_dir, r.path AS repo_path \
           FROM claude_sessions cs \
           LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
           LEFT JOIN repos r ON r.name = ps.repo \
          WHERE cs.session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_optional(pool)
    .await
    .context("load projection file-touch context")?;

    let Some(row) = row else {
        return Ok(None);
    };
    let (Some(repo_name), Some(working_dir)) = (row.repo, row.working_dir) else {
        return Ok(None);
    };
    let repo_root = row.repo_path.unwrap_or_else(|| working_dir.clone());
    Ok(Some(FileTouchContext {
        repo_name,
        repo_root: PathBuf::from(repo_root),
        working_dir: PathBuf::from(working_dir),
    }))
}

async fn load_projection_source_events(
    pool: &Pool,
    session_uuid: Uuid,
) -> anyhow::Result<Vec<super::timeline::StoredEvent>> {
    let mut events = load_all_session_events(pool, session_uuid)
        .await
        .context("load root projection events")?;
    let mut known_ids: HashSet<String> = events
        .iter()
        .filter_map(|event| event.event_uuid.clone())
        .collect();

    for descendant in descendant_session_ids(pool, session_uuid).await? {
        let descendant_events = load_all_session_events(pool, descendant)
            .await
            .with_context(|| format!("load descendant projection events for {descendant}"))?;
        let filtered =
            filter_descendant_projection_events(&descendant_events, &known_ids, descendant);
        known_ids.extend(filtered.iter().filter_map(|event| event.event_uuid.clone()));
        events.extend(filtered);
    }

    events.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.byte_offset.cmp(&right.byte_offset))
    });
    Ok(events)
}

async fn descendant_session_ids(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<Vec<Uuid>> {
    let rows: Vec<(Uuid, i32)> = sqlx::query_as(
        "WITH RECURSIVE descendants AS ( \
             SELECT session_uuid, 0::INT AS depth \
               FROM claude_sessions \
              WHERE session_uuid = $1 \
             UNION ALL \
             SELECT child.session_uuid, descendants.depth + 1 \
               FROM claude_sessions child \
               JOIN descendants ON child.parent_session_uuid = descendants.session_uuid \
         ) \
         SELECT session_uuid, depth \
           FROM descendants \
          WHERE depth > 0 \
          ORDER BY depth ASC, session_uuid ASC",
    )
    .bind(session_uuid)
    .fetch_all(pool)
    .await
    .context("load descendant projection sessions")?;
    Ok(rows.into_iter().map(|(uuid, _)| uuid).collect())
}

fn filter_descendant_projection_events(
    events: &[super::timeline::StoredEvent],
    known_ids: &HashSet<String>,
    descendant_session_uuid: Uuid,
) -> Vec<super::timeline::StoredEvent> {
    let descendant_session_id = descendant_session_uuid.to_string();
    let mut included_ids: HashSet<String> = events
        .iter()
        .filter(|event| event.is_sidechain)
        .filter(|event| event.parent_event_uuid.as_deref() == Some(descendant_session_id.as_str()))
        .filter_map(|event| event.event_uuid.clone())
        .filter(|id| !known_ids.contains(id))
        .collect();

    let mut changed = true;
    while changed {
        changed = false;
        for event in events {
            let parent_included = event
                .parent_event_uuid
                .as_ref()
                .map(|parent| included_ids.contains(parent))
                .unwrap_or(false);
            if !parent_included {
                continue;
            }
            if let Some(event_id) = &event.event_uuid {
                changed |= included_ids.insert(event_id.clone());
            }
        }
    }

    events
        .iter()
        .filter(|event| {
            event
                .event_uuid
                .as_ref()
                .map(|id| included_ids.contains(id))
                .unwrap_or(false)
                || event
                    .parent_event_uuid
                    .as_ref()
                    .map(|parent| included_ids.contains(parent))
                    .unwrap_or(false)
        })
        .cloned()
        .collect()
}

pub async fn backfill_timeline_projection(pool: &Pool) -> anyhow::Result<usize> {
    let sessions: Vec<(Uuid,)> =
        sqlx::query_as("SELECT DISTINCT session_uuid FROM events ORDER BY session_uuid ASC")
            .fetch_all(pool)
            .await
            .context("list sessions for projection backfill")?;

    for (session_uuid,) in &sessions {
        rebuild_session_projection(pool, *session_uuid).await?;
    }
    Ok(sessions.len())
}

fn empty_timeline_response(total_event_count: i64) -> TimelineResponse {
    TimelineResponse {
        session_uuid: None,
        session_agent: None,
        total_event_count,
        turns: Vec::new(),
    }
}

#[derive(Debug, Clone)]
pub struct TimelineSessionMeta {
    pub pty_session_id: Option<Uuid>,
    pub session_uuid: Uuid,
    pub session_agent: Option<String>,
    pub session_label: Option<String>,
    pub session_state: Option<String>,
}

#[derive(FromRow)]
struct SessionMetaRow {
    pty_session_id: Option<Uuid>,
    session_uuid: Uuid,
    session_agent: Option<String>,
    session_label: Option<String>,
    session_state: Option<String>,
}

pub async fn load_timeline_session_meta(
    pool: &Pool,
    session_uuid: Uuid,
) -> anyhow::Result<TimelineSessionMeta> {
    let row: SessionMetaRow = sqlx::query_as(
        "SELECT cs.pty_session_id AS pty_session_id, \
                cs.session_uuid AS session_uuid, \
                cs.agent AS session_agent, \
                ps.label AS session_label, \
                ps.state AS session_state \
           FROM claude_sessions cs \
           LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
          WHERE cs.session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_one(pool)
    .await
    .context("load timeline session metadata")?;

    Ok(TimelineSessionMeta {
        pty_session_id: row.pty_session_id,
        session_uuid: row.session_uuid,
        session_agent: row.session_agent,
        session_label: row.session_label,
        session_state: row.session_state,
    })
}

pub async fn load_repo_timeline_response(
    pool: &Pool,
    repo_name: &str,
    filters: &ProjectionFilters,
) -> anyhow::Result<TimelineResponse> {
    let rows: Vec<SessionMetaRow> = sqlx::query_as(
        "SELECT cs.pty_session_id AS pty_session_id, \
                cs.session_uuid AS session_uuid, \
                cs.agent AS session_agent, \
                ps.label AS session_label, \
                ps.state AS session_state \
           FROM claude_sessions cs \
           JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
          WHERE ps.repo = $1 \
          ORDER BY cs.started_at ASC, cs.session_uuid ASC",
    )
    .bind(repo_name)
    .fetch_all(pool)
    .await
    .context("load repo timeline sessions")?;

    let mut total_event_count = 0_i64;
    let mut turns = Vec::new();
    for row in rows {
        let meta = TimelineSessionMeta {
            pty_session_id: row.pty_session_id,
            session_uuid: row.session_uuid,
            session_agent: row.session_agent,
            session_label: row.session_label,
            session_state: row.session_state,
        };
        let mut response = load_timeline_response(pool, meta.session_uuid, filters).await?;
        total_event_count += response.total_event_count;
        annotate_timeline_turns(&mut response.turns, &meta);
        turns.extend(response.turns);
    }

    turns.sort_by(|left, right| {
        left.start_timestamp
            .cmp(&right.start_timestamp)
            .then_with(|| left.end_timestamp.cmp(&right.end_timestamp))
            .then_with(|| left.session_uuid.cmp(&right.session_uuid))
            .then_with(|| left.id.cmp(&right.id))
    });

    Ok(TimelineResponse {
        session_uuid: None,
        session_agent: None,
        total_event_count,
        turns,
    })
}

pub fn annotate_timeline_turns(turns: &mut [TimelineTurn], meta: &TimelineSessionMeta) {
    for turn in turns {
        turn.turn_key = Some(format!("{}:{}", meta.session_uuid, turn.id));
        turn.pty_session_id = meta.pty_session_id;
        turn.session_uuid = Some(meta.session_uuid);
        turn.session_agent = meta.session_agent.clone();
        turn.session_label = meta.session_label.clone();
        turn.session_state = meta.session_state.clone();
    }
}

async fn load_referenced_turn_ids(
    pool: &Pool,
    session_uuid: Uuid,
    file_path: &str,
) -> anyhow::Result<Option<HashSet<i64>>> {
    if file_path.trim().is_empty() {
        return Ok(None);
    }

    let needle = format!("%{}%", file_path.to_lowercase());
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT turn_id \
           FROM timeline_file_touches \
          WHERE session_uuid = $1 AND LOWER(repo_rel_path) ILIKE $2",
    )
    .bind(session_uuid)
    .bind(needle)
    .fetch_all(pool)
    .await
    .context("load projected timeline file touches")?;

    Ok(Some(
        rows.into_iter()
            .map(|(turn_id,)| turn_id)
            .collect::<HashSet<_>>(),
    ))
}

async fn load_projected_turn_rows(
    pool: &Pool,
    session_uuid: Uuid,
) -> anyhow::Result<Vec<ProjectedTurnRow>> {
    sqlx::query_as(
        "SELECT turn_id, preview, user_prompt_text, start_timestamp, end_timestamp, duration_ms, \
                event_count, operation_count, thinking_count, has_errors, markdown, \
                chunks_json, is_sidechain_turn \
           FROM timeline_turns \
          WHERE session_uuid = $1 \
          ORDER BY turn_ord ASC",
    )
    .bind(session_uuid)
    .fetch_all(pool)
    .await
    .context("load projected timeline turns")
}

async fn load_projected_operation_rows(
    pool: &Pool,
    session_uuid: Uuid,
) -> anyhow::Result<Vec<ProjectedOperationRow>> {
    sqlx::query_as(
        "SELECT turn_id, operation_ord, pair_id, name, raw_name, operation_type, \
                operation_category, input, result_content, result_payload, result_is_error, \
                is_error, is_pending, subagent_json \
           FROM timeline_operations \
          WHERE session_uuid = $1 \
          ORDER BY turn_id ASC, operation_ord ASC",
    )
    .bind(session_uuid)
    .fetch_all(pool)
    .await
    .context("load projected timeline operations")
}

async fn load_projected_touch_rows(
    pool: &Pool,
    session_uuid: Uuid,
) -> anyhow::Result<Vec<ProjectedTouchRow>> {
    sqlx::query_as(
        "SELECT turn_id, operation_ord, repo_name, repo_rel_path, touch_kind, is_write \
           FROM timeline_file_touches \
          WHERE session_uuid = $1 \
          ORDER BY turn_id ASC, touch_ord ASC",
    )
    .bind(session_uuid)
    .fetch_all(pool)
    .await
    .context("load projected timeline file touches")
}

fn build_operations_by_turn(
    operation_rows: Vec<ProjectedOperationRow>,
    touch_rows: Vec<ProjectedTouchRow>,
) -> anyhow::Result<HashMap<i64, Vec<TimelineToolPair>>> {
    let mut touches_by_operation: HashMap<(i64, i32), Vec<TimelineFileTouch>> = HashMap::new();
    for row in touch_rows {
        let Some(operation_ord) = row.operation_ord else {
            continue;
        };
        touches_by_operation
            .entry((row.turn_id, operation_ord))
            .or_default()
            .push(TimelineFileTouch {
                repo: row.repo_name,
                path: row.repo_rel_path,
                touch_kind: row.touch_kind,
                is_write: row.is_write,
            });
    }

    let mut operations_by_turn: HashMap<i64, Vec<TimelineToolPair>> = HashMap::new();
    for row in operation_rows {
        let turn_id = row.turn_id;
        let pair = build_operation_pair(row, &mut touches_by_operation)?;
        operations_by_turn.entry(turn_id).or_default().push(pair);
    }
    Ok(operations_by_turn)
}

fn build_operation_pair(
    row: ProjectedOperationRow,
    touches_by_operation: &mut HashMap<(i64, i32), Vec<TimelineFileTouch>>,
) -> anyhow::Result<TimelineToolPair> {
    let ProjectedOperationRow {
        turn_id,
        operation_ord,
        pair_id,
        name,
        raw_name,
        operation_type,
        operation_category,
        input,
        result_content,
        result_payload,
        result_is_error,
        is_error,
        is_pending,
        subagent_json,
    } = row;

    let subagent = subagent_json
        .map(serde_json::from_value::<TimelineSubagent>)
        .transpose()
        .with_context(|| format!("deserialize projected subagent for {pair_id}"))?
        .map(Box::new);
    let result = if result_content.is_some() || result_payload.is_some() {
        Some(TimelineToolResult {
            content: result_content,
            payload: result_payload,
            is_error: result_is_error,
        })
    } else {
        None
    };

    Ok(TimelineToolPair {
        id: pair_id,
        name,
        raw_name,
        operation_type,
        category: operation_category
            .as_deref()
            .and_then(OperationCategory::parse),
        input,
        result,
        is_error,
        is_pending,
        file_touches: touches_by_operation
            .remove(&(turn_id, operation_ord))
            .unwrap_or_default(),
        subagent,
    })
}

fn build_projected_turn(
    row: ProjectedTurnRow,
    operations_by_turn: &mut HashMap<i64, Vec<TimelineToolPair>>,
) -> anyhow::Result<TimelineTurn> {
    let turn_id = row.turn_id;
    Ok(TimelineTurn {
        id: turn_id,
        turn_key: None,
        preview: row.preview,
        user_prompt_text: row.user_prompt_text,
        start_timestamp: row.start_timestamp,
        end_timestamp: row.end_timestamp,
        duration_ms: row.duration_ms,
        event_count: row.event_count.max(0) as usize,
        operation_count: row.operation_count.max(0) as usize,
        tool_pairs: operations_by_turn.remove(&turn_id).unwrap_or_default(),
        thinking_count: row.thinking_count.max(0) as usize,
        has_errors: row.has_errors,
        markdown: row.markdown,
        chunks: serde_json::from_value(row.chunks_json)
            .with_context(|| format!("deserialize projected timeline chunks for turn {turn_id}"))?,
        pty_session_id: None,
        session_uuid: None,
        session_agent: None,
        session_label: None,
        session_state: None,
    })
}

pub async fn load_timeline_response(
    pool: &Pool,
    session_uuid: Uuid,
    filters: &ProjectionFilters,
) -> anyhow::Result<TimelineResponse> {
    let (total_event_count,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(event_count), 0)::BIGINT FROM timeline_turns WHERE session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_one(pool)
    .await
    .context("count projected timeline events")?;

    let referenced_turn_ids =
        load_referenced_turn_ids(pool, session_uuid, &filters.file_path).await?;
    if referenced_turn_ids.as_ref().is_some_and(HashSet::is_empty) {
        return Ok(empty_timeline_response(total_event_count));
    }

    let rows = load_projected_turn_rows(pool, session_uuid).await?;
    let operation_rows = load_projected_operation_rows(pool, session_uuid).await?;
    let touch_rows = load_projected_touch_rows(pool, session_uuid).await?;
    let mut operations_by_turn = build_operations_by_turn(operation_rows, touch_rows)?;

    let mut turns = Vec::new();
    for row in rows {
        if !filters.show_sidechain && row.is_sidechain_turn {
            continue;
        }
        if let Some(allowed) = &referenced_turn_ids {
            if !allowed.contains(&row.turn_id) {
                continue;
            }
        }
        if filters.errors_only && !row.has_errors {
            continue;
        }
        let mut turn = build_projected_turn(row, &mut operations_by_turn)?;
        apply_projection_filters(&mut turn, filters);
        turns.push(turn);
    }

    Ok(TimelineResponse {
        session_uuid: None,
        session_agent: None,
        total_event_count,
        turns,
    })
}

pub async fn load_repo_file_trace(
    pool: &Pool,
    repo_name: &str,
    repo_rel_path: &str,
) -> anyhow::Result<Vec<RepoFileTraceTouch>> {
    #[derive(FromRow)]
    struct TraceRow {
        pty_session_id: Option<Uuid>,
        session_uuid: Uuid,
        session_agent: Option<String>,
        session_label: Option<String>,
        session_state: Option<String>,
        turn_id: i64,
        turn_preview: String,
        turn_timestamp: DateTime<Utc>,
        operation_type: Option<String>,
        operation_category: Option<String>,
        pair_id: Option<String>,
        touch_kind: String,
        is_write: bool,
    }

    let rows: Vec<TraceRow> = sqlx::query_as(
        "SELECT cs.pty_session_id AS pty_session_id, \
                tf.session_uuid, \
                cs.agent AS session_agent, \
                ps.label AS session_label, \
                ps.state AS session_state, \
                tf.turn_id, \
                tt.preview AS turn_preview, \
                tt.start_timestamp AS turn_timestamp, \
                op.operation_type AS operation_type, \
                op.operation_category AS operation_category, \
                op.pair_id AS pair_id, \
                tf.touch_kind, \
                tf.is_write \
           FROM timeline_file_touches tf \
           JOIN timeline_turns tt \
             ON tt.session_uuid = tf.session_uuid AND tt.turn_id = tf.turn_id \
           LEFT JOIN timeline_operations op \
             ON op.session_uuid = tf.session_uuid \
            AND op.turn_id = tf.turn_id \
            AND op.operation_ord = tf.operation_ord \
           JOIN claude_sessions cs ON cs.session_uuid = tf.session_uuid \
           LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
          WHERE tf.repo_name = $1 AND tf.repo_rel_path = $2 \
          ORDER BY tt.start_timestamp DESC, tf.turn_id DESC, tf.touch_ord ASC",
    )
    .bind(repo_name)
    .bind(repo_rel_path)
    .fetch_all(pool)
    .await
    .context("load repo file trace")?;

    Ok(rows
        .into_iter()
        .map(|row| RepoFileTraceTouch {
            pty_session_id: row.pty_session_id,
            session_uuid: row.session_uuid,
            session_agent: row.session_agent,
            session_label: row.session_label,
            session_state: row.session_state,
            turn_id: row.turn_id,
            turn_preview: row.turn_preview,
            turn_timestamp: row.turn_timestamp,
            operation_type: row.operation_type,
            operation_category: row.operation_category,
            pair_id: row.pair_id,
            touch_kind: row.touch_kind,
            is_write: row.is_write,
        })
        .collect())
}

fn apply_projection_filters(turn: &mut TimelineTurn, filters: &ProjectionFilters) {
    let pair_by_id: HashMap<&str, &TimelineToolPair> = turn
        .tool_pairs
        .iter()
        .map(|pair| (pair.id.as_str(), pair))
        .collect();
    turn.chunks = turn
        .chunks
        .clone()
        .into_iter()
        .filter_map(|chunk| filter_chunk(chunk, &pair_by_id, filters))
        .collect();
}

fn filter_chunk(
    chunk: TimelineChunk,
    pair_by_id: &HashMap<&str, &TimelineToolPair>,
    filters: &ProjectionFilters,
) -> Option<TimelineChunk> {
    match chunk {
        TimelineChunk::Assistant { items, thinking } => {
            if filters.hidden_speakers.contains(&SpeakerFacet::Assistant) {
                return None;
            }
            let items = items
                .into_iter()
                .filter_map(|item| match item {
                    TimelineAssistantItem::Text { .. } => Some(item),
                    TimelineAssistantItem::Tool { pair_id } => pair_by_id
                        .get(pair_id.as_str())
                        .filter(|pair| pair_visible(pair, filters))
                        .map(|_| TimelineAssistantItem::Tool { pair_id }),
                })
                .collect::<Vec<_>>();
            if items.is_empty() && thinking.is_empty() {
                None
            } else {
                Some(TimelineChunk::Assistant { items, thinking })
            }
        }
        TimelineChunk::Tool { pair_id } => {
            if filters.hidden_speakers.contains(&SpeakerFacet::Assistant) {
                return None;
            }
            pair_by_id
                .get(pair_id.as_str())
                .filter(|pair| pair_visible(pair, filters))
                .map(|_| TimelineChunk::Tool { pair_id })
        }
        TimelineChunk::System {
            subtype,
            text,
            is_meta,
        } => {
            if !filters.show_bookkeeping && is_meta {
                None
            } else {
                Some(TimelineChunk::System {
                    subtype,
                    text,
                    is_meta,
                })
            }
        }
        TimelineChunk::Generic { label, details } => {
            if !filters.show_bookkeeping && BOOKKEEPING_KINDS.contains(&label.as_str()) {
                None
            } else {
                Some(TimelineChunk::Generic { label, details })
            }
        }
        other => Some(other),
    }
}

fn pair_visible(pair: &TimelineToolPair, filters: &ProjectionFilters) -> bool {
    pair.category
        .map(|category| !filters.hidden_operation_categories.contains(&category))
        .unwrap_or(true)
}
