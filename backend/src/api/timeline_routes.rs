use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::routes::{repo_path, ApiError, ApiResult};
use crate::ingest::{self, canonical, timeline};
use crate::AppState;

#[derive(Deserialize)]
pub(super) struct TimelineQuery {
    #[serde(default)]
    session: Option<Uuid>,
    #[serde(default)]
    claude_session: Option<Uuid>,
    #[serde(default)]
    hide_speakers: Option<String>,
    #[serde(default)]
    hide_categories: Option<String>,
    #[serde(default)]
    errors_only: Option<bool>,
    #[serde(default)]
    show_bookkeeping: Option<bool>,
    #[serde(default)]
    show_sidechain: Option<bool>,
    #[serde(default)]
    file_path: Option<String>,
}

pub(super) async fn session_timeline(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Json<timeline::TimelineSummaryResponse>> {
    let resolved =
        timeline::resolve_session_target(&state.pool, id, q.session.or(q.claude_session)).await?;

    let resolved = match resolved {
        timeline::SessionLookup::Resolved(resolved) => resolved,
        timeline::SessionLookup::NoSession => {
            return Ok(Json(timeline::TimelineSummaryResponse {
                session_uuid: None,
                session_agent: None,
                total_event_count: 0,
                turns: Vec::new(),
            }));
        }
        timeline::SessionLookup::MissingPty => return Err(ApiError::NotFound),
    };

    let mut response =
        ingest::load_timeline_summary_response(&state.pool, resolved.session_uuid, &filters_for(q))
            .await?;
    let meta = ingest::load_timeline_session_meta(&state.pool, resolved.session_uuid).await?;
    ingest::annotate_timeline_summaries(&mut response.turns, &meta);
    response.session_uuid = Some(resolved.session_uuid);
    response.session_agent = resolved.session_agent;
    Ok(Json(response))
}

pub(super) async fn session_timeline_turn(
    State(state): State<Arc<AppState>>,
    Path((id, turn_id)): Path<(Uuid, i64)>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Json<timeline::TimelineTurnDetailResponse>> {
    let resolved =
        timeline::resolve_session_target(&state.pool, id, q.session.or(q.claude_session)).await?;

    let resolved = match resolved {
        timeline::SessionLookup::Resolved(resolved) => resolved,
        timeline::SessionLookup::NoSession | timeline::SessionLookup::MissingPty => {
            return Err(ApiError::NotFound);
        }
    };

    let Some(mut turn) = ingest::load_timeline_turn_detail(
        &state.pool,
        resolved.session_uuid,
        turn_id,
        &filters_for(q),
    )
    .await?
    else {
        return Err(ApiError::NotFound);
    };
    let meta = ingest::load_timeline_session_meta(&state.pool, resolved.session_uuid).await?;
    ingest::annotate_timeline_turns(std::slice::from_mut(&mut turn), &meta);
    Ok(Json(timeline::TimelineTurnDetailResponse {
        session_uuid: resolved.session_uuid,
        session_agent: resolved.session_agent,
        turn,
    }))
}

pub(super) async fn repo_timeline(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Json<timeline::TimelineSummaryResponse>> {
    let _ = repo_path(&state, &name)?;
    let response =
        ingest::load_repo_timeline_summary_response(&state.pool, &name, &filters_for(q)).await?;
    Ok(Json(response))
}

pub(super) async fn repo_timeline_turn(
    State(state): State<Arc<AppState>>,
    Path((name, session_uuid, turn_id)): Path<(String, Uuid, i64)>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Json<timeline::TimelineTurnDetailResponse>> {
    let _ = repo_path(&state, &name)?;
    ensure_session_belongs_to_repo(&state, session_uuid, &name).await?;

    let Some(mut turn) =
        ingest::load_timeline_turn_detail(&state.pool, session_uuid, turn_id, &filters_for(q))
            .await?
    else {
        return Err(ApiError::NotFound);
    };
    let meta = ingest::load_timeline_session_meta(&state.pool, session_uuid).await?;
    ingest::annotate_timeline_turns(std::slice::from_mut(&mut turn), &meta);
    Ok(Json(timeline::TimelineTurnDetailResponse {
        session_uuid,
        session_agent: meta.session_agent,
        turn,
    }))
}

#[derive(Deserialize)]
pub(super) struct MonitorTimelineRequest {
    #[serde(default)]
    session_ids: Vec<Uuid>,
    #[serde(default)]
    hidden_speakers: Vec<String>,
    #[serde(default)]
    hidden_operation_categories: Vec<String>,
    #[serde(default)]
    errors_only: bool,
    #[serde(default)]
    show_bookkeeping: bool,
    #[serde(default)]
    show_sidechain: bool,
    #[serde(default)]
    file_path: String,
}

#[derive(Serialize)]
pub(super) struct MonitorTimelineResponse {
    generated_at: DateTime<Utc>,
    sessions: Vec<MonitorSessionTurnView>,
}

#[derive(Serialize)]
pub(super) struct MonitorSessionTurnView {
    pty_session_id: Uuid,
    repo: String,
    label: Option<String>,
    pty_state: String,
    current_session_uuid: Option<Uuid>,
    current_session_agent: Option<String>,
    total_event_count: i64,
    turn: Option<timeline::TimelineTurn>,
}

#[derive(FromRow)]
struct MonitorSessionRow {
    pty_session_id: Uuid,
    repo: String,
    label: Option<String>,
    pty_state: String,
    current_session_uuid: Option<Uuid>,
    current_session_agent: Option<String>,
}

pub(super) async fn monitor_timeline(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MonitorTimelineRequest>,
) -> ApiResult<Json<MonitorTimelineResponse>> {
    if req.session_ids.len() > 64 {
        return Err(ApiError::BadRequest(
            "monitor supports at most 64 sessions".into(),
        ));
    }

    let filters = monitor_filters_for(&req);
    let mut rows = load_monitor_session_rows(&state, &req.session_ids).await?;
    rows.sort_by_key(|row| {
        req.session_ids
            .iter()
            .position(|id| *id == row.pty_session_id)
            .unwrap_or(usize::MAX)
    });

    let mut sessions = Vec::with_capacity(rows.len());
    for row in rows {
        let mut total_event_count = 0;
        let turn = match row.current_session_uuid {
            Some(session_uuid) => {
                let summary =
                    ingest::load_timeline_summary_response(&state.pool, session_uuid, &filters)
                        .await?;
                total_event_count = summary.total_event_count;
                match summary.turns.last() {
                    Some(latest) => {
                        let mut turn = ingest::load_timeline_turn_detail(
                            &state.pool,
                            session_uuid,
                            latest.id,
                            &filters,
                        )
                        .await?;
                        if let Some(turn) = turn.as_mut() {
                            let meta =
                                ingest::load_timeline_session_meta(&state.pool, session_uuid)
                                    .await?;
                            ingest::annotate_timeline_turns(std::slice::from_mut(turn), &meta);
                        }
                        turn
                    }
                    None => None,
                }
            }
            None => None,
        };

        sessions.push(MonitorSessionTurnView {
            pty_session_id: row.pty_session_id,
            repo: row.repo,
            label: row.label,
            pty_state: row.pty_state,
            current_session_uuid: row.current_session_uuid,
            current_session_agent: row.current_session_agent,
            total_event_count,
            turn,
        });
    }

    sessions.sort_by(|left, right| match (&left.turn, &right.turn) {
        (Some(left_turn), Some(right_turn)) => right_turn
            .end_timestamp
            .cmp(&left_turn.end_timestamp)
            .then_with(|| left.pty_session_id.cmp(&right.pty_session_id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left.pty_session_id.cmp(&right.pty_session_id),
    });

    Ok(Json(MonitorTimelineResponse {
        generated_at: Utc::now(),
        sessions,
    }))
}

async fn load_monitor_session_rows(
    state: &AppState,
    session_ids: &[Uuid],
) -> ApiResult<Vec<MonitorSessionRow>> {
    if session_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as(
        "SELECT ps.id AS pty_session_id, ps.repo, ps.label, ps.state AS pty_state, \
                ps.current_session_uuid, ps.current_session_agent \
           FROM pty_sessions ps \
          WHERE ps.id = ANY($1) AND ps.state <> 'deleted'",
    )
    .bind(session_ids)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::Db)?;
    Ok(rows)
}

async fn ensure_session_belongs_to_repo(
    state: &AppState,
    session_uuid: Uuid,
    repo_name: &str,
) -> ApiResult<()> {
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT cs.session_uuid \
           FROM claude_sessions cs \
           JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
          WHERE cs.session_uuid = $1 AND ps.repo = $2",
    )
    .bind(session_uuid)
    .bind(repo_name)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::Db)?;
    if exists.is_some() {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

fn filters_for(query: TimelineQuery) -> timeline::ProjectionFilters {
    timeline::ProjectionFilters {
        hidden_speakers: parse_hidden_speakers(query.hide_speakers.as_deref()),
        hidden_operation_categories: parse_hidden_categories(query.hide_categories.as_deref()),
        errors_only: query.errors_only.unwrap_or(false),
        show_bookkeeping: query.show_bookkeeping.unwrap_or(false),
        show_sidechain: query.show_sidechain.unwrap_or(false),
        file_path: query.file_path.unwrap_or_default(),
    }
}

fn monitor_filters_for(query: &MonitorTimelineRequest) -> timeline::ProjectionFilters {
    timeline::ProjectionFilters {
        hidden_speakers: query
            .hidden_speakers
            .iter()
            .filter_map(|value| speaker_facet(value))
            .collect(),
        hidden_operation_categories: query
            .hidden_operation_categories
            .iter()
            .filter_map(|value| canonical::OperationCategory::parse(value))
            .collect(),
        errors_only: query.errors_only,
        show_bookkeeping: query.show_bookkeeping,
        show_sidechain: query.show_sidechain,
        file_path: query.file_path.clone(),
    }
}

fn parse_hidden_speakers(raw: Option<&str>) -> HashSet<timeline::SpeakerFacet> {
    let mut out = HashSet::new();
    for value in raw.unwrap_or_default().split(',').map(str::trim) {
        if let Some(facet) = speaker_facet(value) {
            out.insert(facet);
        }
    }
    out
}

fn speaker_facet(value: &str) -> Option<timeline::SpeakerFacet> {
    match value {
        "user" => Some(timeline::SpeakerFacet::User),
        "assistant" => Some(timeline::SpeakerFacet::Assistant),
        "tool_result" => Some(timeline::SpeakerFacet::ToolResult),
        _ => None,
    }
}

fn parse_hidden_categories(raw: Option<&str>) -> HashSet<canonical::OperationCategory> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter_map(canonical::OperationCategory::parse)
        .collect()
}
