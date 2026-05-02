use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
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

fn parse_hidden_speakers(raw: Option<&str>) -> HashSet<timeline::SpeakerFacet> {
    let mut out = HashSet::new();
    for value in raw.unwrap_or_default().split(',').map(str::trim) {
        match value {
            "user" => {
                out.insert(timeline::SpeakerFacet::User);
            }
            "assistant" => {
                out.insert(timeline::SpeakerFacet::Assistant);
            }
            "tool_result" => {
                out.insert(timeline::SpeakerFacet::ToolResult);
            }
            _ => {}
        }
    }
    out
}

fn parse_hidden_categories(raw: Option<&str>) -> HashSet<canonical::OperationCategory> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter_map(canonical::OperationCategory::parse)
        .collect()
}
