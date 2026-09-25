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
    compose_turn_markdown, subagent_title, FileTouchContext, ProjectionFilters, TimelineFileTouch,
    TimelineItem, TimelineOperationBadge, TimelineResponse, TimelineSubagent,
    TimelineSummaryResponse, TimelineToolPair, TimelineToolResult, TimelineTurn,
    TimelineTurnSummary,
};

mod file_trace;
mod filters;
mod store;
pub mod stream;
mod view;
mod write;

pub use view::{load_session_turns, load_timeline_turn_view, TurnView};

pub use file_trace::{load_repo_file_trace, RepoFileTraceTouch};
use filters::apply_projection_filters;
pub use write::{
    backfill_timeline_projection, project_batch, project_until_caught_up,
    rebuild_session_projection, request_rebuild, session_projection_current, store_turn_digests,
    BATCH_EVENTS, REDUCER_VERSION,
};

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
    /// The stored digest; empty until the archive purge writes it.
    markdown: String,
    is_sidechain_turn: bool,
    input_tokens: i64,
    output_tokens: i64,
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
    is_sidechain_turn: bool,
    input_tokens: i64,
    output_tokens: i64,
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
}

#[derive(FromRow)]
struct ProjectedOperationBadgeRow {
    turn_id: i64,
    name: String,
    operation_type: Option<String>,
    count: i64,
    pending_count: i64,
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
        archived_at: None,
    }
}

#[derive(Debug, Clone)]
pub struct TimelineSessionMeta {
    pub pty_session_id: Option<Uuid>,
    pub session_uuid: Uuid,
    pub session_agent: Option<String>,
    pub session_label: Option<String>,
    pub session_state: Option<String>,
    /// Purged down to its turn digest by the archive loop: turns carry
    /// markdown and files but no operations or chunks until restored.
    pub archived_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
struct SessionMetaRow {
    pty_session_id: Option<Uuid>,
    session_uuid: Uuid,
    session_agent: Option<String>,
    session_label: Option<String>,
    session_state: Option<String>,
    archived_at: Option<DateTime<Utc>>,
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
                ps.state AS session_state, \
                CASE WHEN cs.purged_at IS NOT NULL THEN cs.archived_at ELSE NULL END AS archived_at \
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
        archived_at: row.archived_at,
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
                ps.state AS session_state, \
                CASE WHEN cs.purged_at IS NOT NULL THEN cs.archived_at ELSE NULL END AS archived_at \
           FROM claude_sessions cs \
           JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
          WHERE ps.repo = $1 \
            AND NOT EXISTS ( \
                SELECT 1 \
                  FROM events meta \
                 WHERE meta.session_uuid = cs.session_uuid \
                   AND meta.agent = 'codex' \
                   AND meta.kind = 'session_meta' \
                   AND meta.payload #> '{payload,source,subagent}' IS NOT NULL \
            ) \
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
            archived_at: row.archived_at,
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
        archived_at: None,
    })
}

/// Annotates the session's own turns. Child-session turns shown beside them
/// already carry their own session.
pub fn annotate_timeline_summaries(turns: &mut [TimelineTurnSummary], meta: &TimelineSessionMeta) {
    for turn in turns.iter_mut().filter(|turn| {
        turn.session_uuid
            .is_none_or(|uuid| uuid == meta.session_uuid)
    }) {
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
                is_sidechain_turn, input_tokens, output_tokens \
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
                event_count, operation_count, thinking_count, has_errors, is_sidechain_turn, \
                input_tokens, output_tokens \
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
                is_sidechain_turn, input_tokens, output_tokens \
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
                is_error, is_pending \
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

async fn load_projected_items(
    pool: &Pool,
    session_uuid: Uuid,
    turn_ids: &[i64],
) -> anyhow::Result<HashMap<i64, Vec<TimelineItem>>> {
    if turn_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(i64, i64, Value)> = sqlx::query_as(
        "SELECT turn_id, byte_offset, body FROM timeline_items \
          WHERE session_uuid = $1 AND turn_id = ANY($2) \
          ORDER BY turn_id ASC, byte_offset ASC",
    )
    .bind(session_uuid)
    .bind(turn_ids)
    .fetch_all(pool)
    .await
    .context("load projected timeline items")?;
    let mut items: HashMap<i64, Vec<TimelineItem>> = HashMap::new();
    for (turn_id, offset, body) in rows {
        let chunk = serde_json::from_value(body)
            .with_context(|| format!("decode timeline item of turn {turn_id}"))?;
        items
            .entry(turn_id)
            .or_default()
            .push(TimelineItem { offset, chunk });
    }
    Ok(items)
}

async fn load_projected_operation_badge_rows(
    pool: &Pool,
    session_uuid: Uuid,
    turn_ids: &[i64],
    hidden_operation_categories: &HashSet<OperationCategory>,
) -> anyhow::Result<Vec<ProjectedOperationBadgeRow>> {
    if turn_ids.is_empty() {
        return Ok(Vec::new());
    }
    let hidden = hidden_operation_categories
        .iter()
        .map(|category| category.as_str().to_string())
        .collect::<Vec<_>>();
    sqlx::query_as(
        "SELECT turn_id, COALESCE(operation_type, name) AS name, operation_type, \
                COUNT(*)::BIGINT AS count, \
                COUNT(*) FILTER (WHERE is_pending)::BIGINT AS pending_count \
           FROM timeline_operations \
          WHERE session_uuid = $1 AND turn_id = ANY($2) \
            AND (operation_category IS NULL OR operation_category != ALL($3)) \
          GROUP BY turn_id, COALESCE(operation_type, name), operation_type \
          ORDER BY turn_id ASC, count DESC, name ASC",
    )
    .bind(session_uuid)
    .bind(turn_ids)
    .bind(&hidden)
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
                pending_count: row.pending_count.max(0) as usize,
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
    } = row;

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
        subagent: None,
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
        is_sidechain: row.is_sidechain_turn,
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        pty_session_id: None,
        session_uuid: None,
        session_agent: None,
        session_label: None,
        session_state: None,
    }
}

/// A stored turn with its items and operations. The digest is composed
/// from them unless the archive purge already stored it.
fn build_projected_turn(
    row: ProjectedTurnRow,
    operations_by_turn: &mut HashMap<i64, Vec<TimelineToolPair>>,
    items_by_turn: &mut HashMap<i64, Vec<TimelineItem>>,
) -> TimelineTurn {
    let turn_id = row.turn_id;
    let tool_pairs = operations_by_turn.remove(&turn_id).unwrap_or_default();
    let items = items_by_turn.remove(&turn_id).unwrap_or_default();
    let markdown = if row.markdown.is_empty() {
        compose_turn_markdown(row.user_prompt_text.as_deref(), &items, &tool_pairs)
    } else {
        row.markdown
    };
    TimelineTurn {
        id: turn_id,
        turn_key: None,
        preview: row.preview,
        user_prompt_text: row.user_prompt_text,
        start_timestamp: row.start_timestamp,
        end_timestamp: row.end_timestamp,
        duration_ms: row.duration_ms,
        event_count: row.event_count.max(0) as usize,
        operation_count: row.operation_count.max(0) as usize,
        tool_pairs,
        thinking_count: row.thinking_count.max(0) as usize,
        has_errors: row.has_errors,
        is_sidechain: row.is_sidechain_turn,
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        markdown,
        items,
        pty_session_id: None,
        session_uuid: None,
        session_agent: None,
        session_label: None,
        session_state: None,
    }
}

/// The session's turn summaries. The sidechain view adds the turns of the
/// sessions it spawned, read from their own timelines.
pub async fn load_timeline_summary_response(
    pool: &Pool,
    session_uuid: Uuid,
    filters: &ProjectionFilters,
) -> anyhow::Result<TimelineSummaryResponse> {
    let mut response = load_own_timeline_summary(pool, session_uuid, filters).await?;
    if filters.show_sidechain {
        response
            .turns
            .extend(view::load_child_turn_summaries(pool, session_uuid, filters).await?);
        response.turns.sort_by(|left, right| {
            left.start_timestamp
                .cmp(&right.start_timestamp)
                .then_with(|| left.session_uuid.cmp(&right.session_uuid))
                .then_with(|| left.id.cmp(&right.id))
        });
    }
    Ok(response)
}

async fn load_own_timeline_summary(
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
    let badge_rows = load_projected_operation_badge_rows(
        pool,
        session_uuid,
        &turn_ids,
        &filters.hidden_operation_categories,
    )
    .await?;
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
        archived_at: None,
    })
}

pub async fn load_timeline_turn_detail(
    pool: &Pool,
    session_uuid: Uuid,
    turn_id: i64,
    filters: &ProjectionFilters,
) -> anyhow::Result<Option<TimelineTurn>> {
    Ok(
        load_timeline_turn_view(pool, session_uuid, turn_id, filters, None)
            .await?
            .map(|view| view.turn),
    )
}

/// Turns with their items, operations, file touches and child references.
async fn load_full_turns(
    pool: &Pool,
    session_uuid: Uuid,
    rows: Vec<ProjectedTurnRow>,
    turn_ids: &[i64],
) -> anyhow::Result<Vec<TimelineTurn>> {
    let operation_rows = load_projected_operation_rows(pool, session_uuid, turn_ids).await?;
    let touch_rows = load_projected_touch_rows(pool, session_uuid, turn_ids).await?;
    let mut operations_by_turn = build_operations_by_turn(operation_rows, touch_rows)?;
    let mut items_by_turn = load_projected_items(pool, session_uuid, turn_ids).await?;
    let mut turns: Vec<TimelineTurn> = rows
        .into_iter()
        .map(|row| build_projected_turn(row, &mut operations_by_turn, &mut items_by_turn))
        .collect();
    let mut pairs: Vec<&mut TimelineToolPair> = turns
        .iter_mut()
        .flat_map(|turn| turn.tool_pairs.iter_mut())
        .collect();
    view::attach_subagents(pool, session_uuid, &mut pairs).await?;
    Ok(turns)
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
    let rows: Vec<ProjectedTurnRow> = rows
        .into_iter()
        .filter(|row| filters.show_sidechain || !row.is_sidechain_turn)
        .filter(|row| {
            referenced_turn_ids
                .as_ref()
                .is_none_or(|allowed| allowed.contains(&row.turn_id))
        })
        .filter(|row| !filters.errors_only || row.has_errors)
        .collect();
    let turn_ids = rows.iter().map(|row| row.turn_id).collect::<Vec<_>>();
    let mut turns = load_full_turns(pool, session_uuid, rows, &turn_ids).await?;
    for turn in &mut turns {
        apply_projection_filters(turn, filters);
    }

    Ok(TimelineResponse {
        session_uuid: None,
        session_agent: None,
        total_event_count,
        turns,
    })
}
