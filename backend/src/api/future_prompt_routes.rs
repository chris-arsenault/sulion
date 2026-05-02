use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use super::routes::{ApiError, ApiResult};
use crate::future_prompts;
use crate::ingest::timeline;
use crate::AppState;

#[derive(Serialize)]
pub(super) struct FuturePromptListResponse {
    session_uuid: Option<Uuid>,
    session_agent: Option<String>,
    prompts: Vec<future_prompts::FuturePromptEntry>,
}

pub(super) async fn list_future_prompts(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<FuturePromptListResponse>> {
    let Some(resolved) = resolve_future_prompt_session(&state, id).await? else {
        return Ok(Json(FuturePromptListResponse {
            session_uuid: None,
            session_agent: None,
            prompts: Vec::new(),
        }));
    };

    let prompts = future_prompts::list(&future_prompts_root(&state), resolved.session_uuid)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(FuturePromptListResponse {
        session_uuid: Some(resolved.session_uuid),
        session_agent: resolved.session_agent,
        prompts,
    }))
}

pub(super) async fn create_future_prompt(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(input): Json<future_prompts::CreateInput>,
) -> ApiResult<(StatusCode, Json<future_prompts::FuturePromptEntry>)> {
    let Some(resolved) = resolve_future_prompt_session(&state, id).await? else {
        return Err(ApiError::BadRequest(
            "no correlated transcript session for this terminal".into(),
        ));
    };

    let entry = future_prompts::create(&future_prompts_root(&state), resolved.session_uuid, input)
        .await
        .map_err(ApiError::Internal)?;
    refresh_future_prompt_count(&state, resolved.session_uuid).await?;
    Ok((StatusCode::CREATED, Json(entry)))
}

pub(super) async fn update_future_prompt(
    State(state): State<Arc<AppState>>,
    Path((id, item_id)): Path<(Uuid, String)>,
    Json(input): Json<future_prompts::UpdateInput>,
) -> ApiResult<Json<future_prompts::FuturePromptEntry>> {
    let Some(resolved) = resolve_future_prompt_session(&state, id).await? else {
        return Err(ApiError::BadRequest(
            "no correlated transcript session for this terminal".into(),
        ));
    };

    let entry = future_prompts::update(
        &future_prompts_root(&state),
        resolved.session_uuid,
        &item_id,
        input,
    )
    .await
    .map_err(ApiError::Internal)?;
    let Some(entry) = entry else {
        return Err(ApiError::NotFound);
    };
    refresh_future_prompt_count(&state, resolved.session_uuid).await?;
    Ok(Json(entry))
}

pub(super) async fn delete_future_prompt(
    State(state): State<Arc<AppState>>,
    Path((id, item_id)): Path<(Uuid, String)>,
) -> ApiResult<StatusCode> {
    let Some(resolved) = resolve_future_prompt_session(&state, id).await? else {
        return Err(ApiError::BadRequest(
            "no correlated transcript session for this terminal".into(),
        ));
    };

    let removed = future_prompts::delete(
        &future_prompts_root(&state),
        resolved.session_uuid,
        &item_id,
    )
    .await
    .map_err(ApiError::Internal)?;
    if !removed {
        return Err(ApiError::NotFound);
    }
    refresh_future_prompt_count(&state, resolved.session_uuid).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn future_prompts_root(state: &AppState) -> PathBuf {
    state
        .library_root
        .parent()
        .unwrap_or(state.library_root.as_path())
        .join("future-prompts")
}

async fn resolve_future_prompt_session(
    state: &AppState,
    pty_id: Uuid,
) -> ApiResult<Option<timeline::ResolvedSession>> {
    let resolved = timeline::resolve_session_target(&state.pool, pty_id, None).await?;
    match resolved {
        timeline::SessionLookup::Resolved(resolved) => Ok(Some(resolved)),
        timeline::SessionLookup::NoSession => Ok(None),
        timeline::SessionLookup::MissingPty => Err(ApiError::NotFound),
    }
}

async fn refresh_future_prompt_count(state: &AppState, session_uuid: Uuid) -> ApiResult<()> {
    let pending = future_prompts::count_pending(&future_prompts_root(state), session_uuid)
        .await
        .map_err(ApiError::Internal)?;
    sqlx::query(
        "INSERT INTO future_prompt_session_state \
             (session_uuid, revision, pending_count, reconciled_at) \
         VALUES ($1, 1, $2, NOW()) \
         ON CONFLICT (session_uuid) DO UPDATE SET \
             revision = future_prompt_session_state.revision + 1, \
             pending_count = EXCLUDED.pending_count, \
             reconciled_at = NOW()",
    )
    .bind(session_uuid)
    .bind(i32::try_from(pending).unwrap_or(i32::MAX))
    .execute(&state.pool)
    .await
    .map_err(ApiError::Db)?;
    Ok(())
}
