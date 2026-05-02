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
    load_all_session_events, FileTouchContext, ProjectionFilters, StoredEvent, TimelineFileTouch,
    TimelineOperationBadge, TimelineResponse, TimelineSubagent, TimelineSummaryResponse,
    TimelineToolPair, TimelineToolResult, TimelineTurn, TimelineTurnSummary,
};

mod file_trace;
mod filters;
mod write;

pub use file_trace::{load_repo_file_trace, RepoFileTraceTouch};
use filters::apply_projection_filters;
pub use write::{rebuild_session_projection, rebuild_session_projection_after_insert};

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
struct ProjectedTurnSummaryRow {
    turn_id: i64,
    preview: String,
    start_timestamp: DateTime<Utc>,
    end_timestamp: DateTime<Utc>,
    duration_ms: i64,
    event_count: i32,
    operation_count: i32,
    thinking_count: i32,
    has_errors: bool,
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
struct ProjectedOperationBadgeRow {
    turn_id: i64,
    name: String,
    operation_type: Option<String>,
    count: i64,
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
) -> anyhow::Result<Vec<StoredEvent>> {
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
    events: &[StoredEvent],
    known_ids: &HashSet<String>,
    descendant_session_uuid: Uuid,
) -> Vec<StoredEvent> {
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
    let sessions: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT e.session_uuid \
           FROM events e \
          WHERE NOT EXISTS ( \
                SELECT 1 \
                  FROM timeline_turns tt \
                 WHERE tt.session_uuid = e.session_uuid \
          ) \
          ORDER BY e.session_uuid ASC",
    )
    .fetch_all(pool)
    .await
    .context("list sessions missing timeline projection")?;

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

fn empty_timeline_summary_response(total_event_count: i64) -> TimelineSummaryResponse {
    TimelineSummaryResponse {
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

pub async fn load_repo_timeline_summary_response(
    pool: &Pool,
    repo_name: &str,
    filters: &ProjectionFilters,
) -> anyhow::Result<TimelineSummaryResponse> {
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
        let mut response = load_timeline_summary_response(pool, meta.session_uuid, filters).await?;
        total_event_count += response.total_event_count;
        annotate_timeline_summaries(&mut response.turns, &meta);
        turns.extend(response.turns);
    }

    turns.sort_by(|left, right| {
        left.start_timestamp
            .cmp(&right.start_timestamp)
            .then_with(|| left.end_timestamp.cmp(&right.end_timestamp))
            .then_with(|| left.session_uuid.cmp(&right.session_uuid))
            .then_with(|| left.id.cmp(&right.id))
    });

    Ok(TimelineSummaryResponse {
        session_uuid: None,
        session_agent: None,
        total_event_count,
        turns,
    })
}

pub fn annotate_timeline_summaries(turns: &mut [TimelineTurnSummary], meta: &TimelineSessionMeta) {
    for turn in turns {
        turn.turn_key = Some(format!("{}:{}", meta.session_uuid, turn.id));
        turn.pty_session_id = meta.pty_session_id;
        turn.session_uuid = Some(meta.session_uuid);
        turn.session_agent = meta.session_agent.clone();
        turn.session_label = meta.session_label.clone();
        turn.session_state = meta.session_state.clone();
    }
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
    referenced_turn_ids: Option<&HashSet<i64>>,
    errors_only: bool,
) -> anyhow::Result<Vec<ProjectedTurnRow>> {
    let turn_ids = referenced_turn_ids
        .map(|ids| ids.iter().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    sqlx::query_as(
        "SELECT turn_id, preview, user_prompt_text, start_timestamp, end_timestamp, duration_ms, \
                event_count, operation_count, thinking_count, has_errors, markdown, \
                chunks_json, is_sidechain_turn \
           FROM timeline_turns \
          WHERE session_uuid = $1 \
            AND ($2 OR turn_id = ANY($3)) \
            AND ($4 = FALSE OR has_errors = TRUE) \
          ORDER BY turn_ord ASC",
    )
    .bind(session_uuid)
    .bind(referenced_turn_ids.is_none())
    .bind(&turn_ids)
    .bind(errors_only)
    .fetch_all(pool)
    .await
    .context("load projected timeline turns")
}

async fn load_projected_turn_summary_rows(
    pool: &Pool,
    session_uuid: Uuid,
    referenced_turn_ids: Option<&HashSet<i64>>,
    filters: &ProjectionFilters,
) -> anyhow::Result<Vec<ProjectedTurnSummaryRow>> {
    let turn_ids = referenced_turn_ids
        .map(|ids| ids.iter().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    sqlx::query_as(
        "SELECT turn_id, preview, start_timestamp, end_timestamp, duration_ms, \
                event_count, operation_count, thinking_count, has_errors \
           FROM timeline_turns \
          WHERE session_uuid = $1 \
            AND ($2 OR turn_id = ANY($3)) \
            AND ($4 = FALSE OR has_errors = TRUE) \
            AND ($5 = TRUE OR is_sidechain_turn = FALSE) \
          ORDER BY turn_ord ASC",
    )
    .bind(session_uuid)
    .bind(referenced_turn_ids.is_none())
    .bind(&turn_ids)
    .bind(filters.errors_only)
    .bind(filters.show_sidechain)
    .fetch_all(pool)
    .await
    .context("load projected timeline turn summaries")
}

async fn load_projected_turn_row(
    pool: &Pool,
    session_uuid: Uuid,
    turn_id: i64,
    errors_only: bool,
) -> anyhow::Result<Option<ProjectedTurnRow>> {
    sqlx::query_as(
        "SELECT turn_id, preview, user_prompt_text, start_timestamp, end_timestamp, duration_ms, \
                event_count, operation_count, thinking_count, has_errors, markdown, \
                chunks_json, is_sidechain_turn \
           FROM timeline_turns \
          WHERE session_uuid = $1 \
            AND turn_id = $2 \
            AND ($3 = FALSE OR has_errors = TRUE)",
    )
    .bind(session_uuid)
    .bind(turn_id)
    .bind(errors_only)
    .fetch_optional(pool)
    .await
    .context("load projected timeline turn")
}

async fn load_projected_operation_rows(
    pool: &Pool,
    session_uuid: Uuid,
    turn_ids: &[i64],
) -> anyhow::Result<Vec<ProjectedOperationRow>> {
    if turn_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT turn_id, operation_ord, pair_id, name, raw_name, operation_type, \
                operation_category, input, result_content, result_payload, result_is_error, \
                is_error, is_pending, subagent_json \
           FROM timeline_operations \
          WHERE session_uuid = $1 AND turn_id = ANY($2) \
          ORDER BY turn_id ASC, operation_ord ASC",
    )
    .bind(session_uuid)
    .bind(turn_ids)
    .fetch_all(pool)
    .await
    .context("load projected timeline operations")
}

async fn load_projected_operation_badge_rows(
    pool: &Pool,
    session_uuid: Uuid,
    turn_ids: &[i64],
) -> anyhow::Result<Vec<ProjectedOperationBadgeRow>> {
    if turn_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT turn_id, COALESCE(operation_type, name) AS name, operation_type, COUNT(*)::BIGINT AS count \
           FROM timeline_operations \
          WHERE session_uuid = $1 AND turn_id = ANY($2) \
          GROUP BY turn_id, COALESCE(operation_type, name), operation_type \
          ORDER BY turn_id ASC, count DESC, name ASC",
    )
    .bind(session_uuid)
    .bind(turn_ids)
    .fetch_all(pool)
    .await
    .context("load projected timeline operation badges")
}

async fn load_projected_touch_rows(
    pool: &Pool,
    session_uuid: Uuid,
    turn_ids: &[i64],
) -> anyhow::Result<Vec<ProjectedTouchRow>> {
    if turn_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT turn_id, operation_ord, repo_name, repo_rel_path, touch_kind, is_write \
           FROM timeline_file_touches \
          WHERE session_uuid = $1 AND turn_id = ANY($2) \
          ORDER BY turn_id ASC, touch_ord ASC",
    )
    .bind(session_uuid)
    .bind(turn_ids)
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

fn build_operation_badges_by_turn(
    rows: Vec<ProjectedOperationBadgeRow>,
) -> HashMap<i64, Vec<TimelineOperationBadge>> {
    let mut badges_by_turn: HashMap<i64, Vec<TimelineOperationBadge>> = HashMap::new();
    for row in rows {
        badges_by_turn
            .entry(row.turn_id)
            .or_default()
            .push(TimelineOperationBadge {
                name: row.name,
                operation_type: row.operation_type,
                count: row.count.max(0) as usize,
            });
    }
    badges_by_turn
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

fn build_projected_turn_summary(
    row: ProjectedTurnSummaryRow,
    badges_by_turn: &mut HashMap<i64, Vec<TimelineOperationBadge>>,
) -> TimelineTurnSummary {
    let turn_id = row.turn_id;
    TimelineTurnSummary {
        id: turn_id,
        turn_key: None,
        preview: row.preview,
        start_timestamp: row.start_timestamp,
        end_timestamp: row.end_timestamp,
        duration_ms: row.duration_ms,
        event_count: row.event_count.max(0) as usize,
        operation_count: row.operation_count.max(0) as usize,
        operation_badges: badges_by_turn.remove(&turn_id).unwrap_or_default(),
        thinking_count: row.thinking_count.max(0) as usize,
        has_errors: row.has_errors,
        pty_session_id: None,
        session_uuid: None,
        session_agent: None,
        session_label: None,
        session_state: None,
    }
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

pub async fn load_timeline_summary_response(
    pool: &Pool,
    session_uuid: Uuid,
    filters: &ProjectionFilters,
) -> anyhow::Result<TimelineSummaryResponse> {
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
        return Ok(empty_timeline_summary_response(total_event_count));
    }

    let rows =
        load_projected_turn_summary_rows(pool, session_uuid, referenced_turn_ids.as_ref(), filters)
            .await?;
    let turn_ids = rows.iter().map(|row| row.turn_id).collect::<Vec<_>>();
    let badge_rows = load_projected_operation_badge_rows(pool, session_uuid, &turn_ids).await?;
    let mut badges_by_turn = build_operation_badges_by_turn(badge_rows);

    let turns = rows
        .into_iter()
        .map(|row| build_projected_turn_summary(row, &mut badges_by_turn))
        .collect();

    Ok(TimelineSummaryResponse {
        session_uuid: None,
        session_agent: None,
        total_event_count,
        turns,
    })
}

pub async fn load_timeline_turn_detail(
    pool: &Pool,
    session_uuid: Uuid,
    turn_id: i64,
    filters: &ProjectionFilters,
) -> anyhow::Result<Option<TimelineTurn>> {
    let referenced_turn_ids =
        load_referenced_turn_ids(pool, session_uuid, &filters.file_path).await?;
    if referenced_turn_ids
        .as_ref()
        .is_some_and(|ids| !ids.contains(&turn_id))
    {
        return Ok(None);
    }

    let Some(row) =
        load_projected_turn_row(pool, session_uuid, turn_id, filters.errors_only).await?
    else {
        return Ok(None);
    };
    if !filters.show_sidechain && row.is_sidechain_turn {
        return Ok(None);
    }

    let turn_ids = [turn_id];
    let operation_rows = load_projected_operation_rows(pool, session_uuid, &turn_ids).await?;
    let touch_rows = load_projected_touch_rows(pool, session_uuid, &turn_ids).await?;
    let mut operations_by_turn = build_operations_by_turn(operation_rows, touch_rows)?;
    let mut turn = build_projected_turn(row, &mut operations_by_turn)?;
    apply_projection_filters(&mut turn, filters);
    Ok(Some(turn))
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

    let rows = load_projected_turn_rows(
        pool,
        session_uuid,
        referenced_turn_ids.as_ref(),
        filters.errors_only,
    )
    .await?;
    let turn_ids = rows.iter().map(|row| row.turn_id).collect::<Vec<_>>();
    let operation_rows = load_projected_operation_rows(pool, session_uuid, &turn_ids).await?;
    let touch_rows = load_projected_touch_rows(pool, session_uuid, &turn_ids).await?;
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
