//! Admin actions. Destructive or support-tier endpoints that don't
//! belong next to the ordinary session/repo surface.

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use super::routes::{ApiError, ApiResult};
use crate::ingest;
use crate::AppState;

/// Response for `POST /api/admin/reindex`. Only reports facts that
/// are true at the moment the DB transaction commits. How many
/// events the ingester subsequently replays from JSONL is a running
/// total visible through `/api/stats` — not something this endpoint
/// can honestly snapshot.
#[derive(Serialize)]
pub(super) struct ReindexResponse {
    /// claude_sessions rows deleted. Cascades wiped every dependent
    /// row (events, event_blocks, timeline_* projections).
    sessions_cleared: u64,
    /// ingester_state rows deleted (per-session file commit offsets).
    /// After this, the ingester re-reads every JSONL from byte 0.
    offsets_cleared: u64,
}

pub(super) async fn reindex(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ReindexResponse>> {
    let stats = ingest::reset_ingest_state(&state.pool)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(ReindexResponse {
        sessions_cleared: stats.sessions_cleared,
        offsets_cleared: stats.offsets_cleared,
    }))
}
