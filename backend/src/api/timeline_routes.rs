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
) -> ApiResult<Json<timeline::TimelineResponse>> {
    let resolved =
        timeline::resolve_session_target(&state.pool, id, q.session.or(q.claude_session)).await?;

    let resolved = match resolved {
        timeline::SessionLookup::Resolved(resolved) => resolved,
        timeline::SessionLookup::NoSession => {
            return Ok(Json(timeline::TimelineResponse {
                session_uuid: None,
                session_agent: None,
                total_event_count: 0,
                turns: Vec::new(),
            }));
        }
        timeline::SessionLookup::MissingPty => return Err(ApiError::NotFound),
    };

    let mut response =
        ingest::load_timeline_response(&state.pool, resolved.session_uuid, &filters_for(q)).await?;
    let meta = ingest::load_timeline_session_meta(&state.pool, resolved.session_uuid).await?;
    ingest::annotate_timeline_turns(&mut response.turns, &meta);
    response.session_uuid = Some(resolved.session_uuid);
    response.session_agent = resolved.session_agent;
    Ok(Json(response))
}

pub(super) async fn repo_timeline(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Json<timeline::TimelineResponse>> {
    let _ = repo_path(&state, &name)?;
    let response = ingest::load_repo_timeline_response(&state.pool, &name, &filters_for(q)).await?;
    Ok(Json(response))
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
