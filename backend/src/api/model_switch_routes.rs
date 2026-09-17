//! Model-switch guard: the routes behind the timeline's switch dialog and
//! the control-process loop that stops a turn running on a model the user
//! has not accepted. Detection lives in the ingester
//! (`crate::model_switches::observe_event`); this side only reads the rows
//! it writes and reaches the node for the interrupt.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::routes::{ApiError, ApiResult};
use crate::model_switches::{self, ModelSwitch, ENFORCE_INTERVAL};
use crate::pty;
use crate::AppState;

#[derive(Serialize)]
pub(super) struct ModelSwitchListResponse {
    switches: Vec<ModelSwitch>,
}

pub(super) async fn list_model_switches(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ModelSwitchListResponse>> {
    pty::read_meta(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let switches = model_switches::list_for_pty(&state.pool, id)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(ModelSwitchListResponse { switches }))
}

#[derive(Deserialize, Default)]
pub(super) struct AcknowledgeReq {
    /// Accept the new model as the session's confirmed model. Without it
    /// the dialog closes but the previous model stays the one the guard
    /// expects, for a user about to switch back in the terminal.
    #[serde(default)]
    adopt: bool,
}

pub(super) async fn acknowledge_model_switch(
    State(state): State<Arc<AppState>>,
    Path((id, switch_id)): Path<(Uuid, Uuid)>,
    body: Option<Json<AcknowledgeReq>>,
) -> ApiResult<StatusCode> {
    let req = body.map(|Json(req)| req).unwrap_or_default();
    let closed = model_switches::acknowledge(&state.pool, id, switch_id, req.adopt)
        .await
        .map_err(ApiError::Internal)?;
    if !closed {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// One enforcement pass: interrupt every live PTY whose running turn is on
/// an unacknowledged model. Returns how many interrupts landed.
pub async fn enforce_once(state: &AppState) -> anyhow::Result<usize> {
    let pending = model_switches::pending_enforcement(&state.pool).await?;
    let mut landed = 0;
    for (switch_id, pty_session_id) in pending {
        match super::session_routes::interrupt_agent(state, pty_session_id).await {
            Ok(()) => {
                model_switches::mark_interrupted(&state.pool, switch_id).await?;
                tracing::info!(
                    switch = %switch_id,
                    pty = %pty_session_id,
                    "interrupted turn running on an unconfirmed model",
                );
                landed += 1;
            }
            Err(err) => {
                let message = err.to_string();
                tracing::warn!(
                    switch = %switch_id,
                    pty = %pty_session_id,
                    error = %message,
                    "model switch interrupt failed; will retry",
                );
                model_switches::mark_interrupt_error(&state.pool, switch_id, &message).await?;
            }
        }
    }
    Ok(landed)
}

/// Control-process loop. Errors are logged and the loop continues; a
/// database blip must not leave a degraded turn running for good.
pub async fn run_model_switch_enforcer(state: Arc<AppState>) {
    loop {
        if let Err(err) = enforce_once(&state).await {
            tracing::warn!(
                error = format!("{err:#}"),
                "model switch enforcement failed"
            );
        }
        tokio::time::sleep(ENFORCE_INTERVAL).await;
    }
}
