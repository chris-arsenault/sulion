use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use super::routes::{ApiError, ApiResult};
use crate::pty;
use crate::submitted_prompts::{self, PromptGate, SubmittedPrompt};
use crate::AppState;

#[derive(Serialize)]
pub(super) struct SubmittedPromptListResponse {
    /// Why the timeline input is closed right now, if it is.
    gate: Option<PromptGate>,
    prompts: Vec<SubmittedPrompt>,
}

pub(super) async fn list_submitted_prompts(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SubmittedPromptListResponse>> {
    pty::read_meta(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    // Match before listing so a prompt the ingester has just projected does
    // not show as unmatched for one more reconciler tick.
    submitted_prompts::reconcile(&state.pool)
        .await
        .map_err(ApiError::Internal)?;
    let gate = submitted_prompts::prompt_gate(&state.pool, id)
        .await
        .map_err(ApiError::Internal)?;
    let prompts = submitted_prompts::list(&state.pool, id)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(SubmittedPromptListResponse { gate, prompts }))
}

pub(super) async fn dismiss_submitted_prompt(
    State(state): State<Arc<AppState>>,
    Path((id, item_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let dismissed = submitted_prompts::dismiss(&state.pool, id, item_id)
        .await
        .map_err(ApiError::Internal)?;
    if !dismissed {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}
