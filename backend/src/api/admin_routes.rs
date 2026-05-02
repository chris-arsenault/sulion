//! Admin actions. Destructive or support-tier endpoints that don't
//! belong next to the ordinary session/repo surface.

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use super::routes::{ApiError, ApiResult};
use crate::ingest;
use crate::AppState;

/// Response for `POST /api/admin/reindex`. This rebuilds derived
/// transcript tables from existing `events.payload` rows; it does not
/// delete source events or ingest offsets.
#[derive(Serialize)]
pub(super) struct ReindexResponse {
    sessions_rebuilt: u64,
    events_preserved: u64,
    canonical_events_rebuilt: u64,
    timeline_sessions_rebuilt: u64,
}

pub(super) async fn reindex(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ReindexResponse>> {
    let stats = ingest::rebuild_ingest_derivatives(&state.pool)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(ReindexResponse {
        sessions_rebuilt: stats.sessions_rebuilt,
        events_preserved: stats.events_preserved,
        canonical_events_rebuilt: stats.canonical_events_rebuilt,
        timeline_sessions_rebuilt: stats.timeline_sessions_rebuilt,
    }))
}
