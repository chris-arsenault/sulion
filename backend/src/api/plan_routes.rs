use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::routes::{validate_repo_name, ApiError, ApiResult};
use crate::plans::{
    self, BranchPlanInput, CreatePlanInput, NewPhase, PlanActor, PlanEventView, PlanTreeNodeView,
    PlanView, UpdatePhaseInput, UpdatePlanInput,
};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub(super) struct ListPlansQuery {
    #[serde(default)]
    include_closed: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateRepoPlanRequest {
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    phases: Vec<NewPhase>,
    #[serde(default)]
    all_pending: bool,
    #[serde(default)]
    attach_pty_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AttachPlanRequest {
    pty_session_id: Uuid,
}

pub(super) async fn list_repo_plans(
    State(state): State<Arc<AppState>>,
    Path(repo_name): Path<String>,
    Query(query): Query<ListPlansQuery>,
) -> ApiResult<Json<Vec<PlanView>>> {
    validate_repo_name(&repo_name)?;
    let plans = plans::list_for_repo(&state.pool, &repo_name, query.include_closed)
        .await
        .map_err(plan_api_error)?;
    Ok(Json(plans))
}

pub(super) async fn create_repo_plan(
    State(state): State<Arc<AppState>>,
    Path(repo_name): Path<String>,
    Json(request): Json<CreateRepoPlanRequest>,
) -> ApiResult<(StatusCode, Json<PlanView>)> {
    validate_repo_name(&repo_name)?;
    let actor = PlanActor::user();
    let plan = plans::create(
        &state.pool,
        CreatePlanInput {
            repo_name,
            title: request.title,
            summary: request.summary,
            phases: request.phases,
            all_pending: request.all_pending,
            attach_current_pty: false,
        },
        &actor,
    )
    .await
    .map_err(plan_api_error)?;
    let plan = if let Some(pty_session_id) = request.attach_pty_id {
        plans::attach(&state.pool, plan.id, pty_session_id, &actor)
            .await
            .map_err(plan_api_error)?
    } else {
        plan
    };
    Ok((StatusCode::CREATED, Json(plan)))
}

pub(super) async fn get_plan(
    State(state): State<Arc<AppState>>,
    Path(plan_id): Path<Uuid>,
) -> ApiResult<Json<PlanView>> {
    plans::get(&state.pool, plan_id)
        .await
        .map(Json)
        .map_err(plan_api_error)
}

pub(super) async fn patch_plan(
    State(state): State<Arc<AppState>>,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<UpdatePlanInput>,
) -> ApiResult<Json<PlanView>> {
    plans::update(&state.pool, plan_id, request, &PlanActor::user())
        .await
        .map(Json)
        .map_err(plan_api_error)
}

pub(super) async fn add_plan_phase(
    State(state): State<Arc<AppState>>,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<NewPhase>,
) -> ApiResult<(StatusCode, Json<PlanView>)> {
    let plan = plans::add_phase(&state.pool, plan_id, request, &PlanActor::user())
        .await
        .map_err(plan_api_error)?;
    Ok((StatusCode::CREATED, Json(plan)))
}

pub(super) async fn patch_plan_phase(
    State(state): State<Arc<AppState>>,
    Path((plan_id, phase_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<UpdatePhaseInput>,
) -> ApiResult<Json<PlanView>> {
    plans::update_phase(&state.pool, plan_id, phase_id, request, &PlanActor::user())
        .await
        .map(Json)
        .map_err(plan_api_error)
}

pub(super) async fn attach_plan(
    State(state): State<Arc<AppState>>,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<AttachPlanRequest>,
) -> ApiResult<Json<PlanView>> {
    plans::attach(
        &state.pool,
        plan_id,
        request.pty_session_id,
        &PlanActor::user(),
    )
    .await
    .map(Json)
    .map_err(plan_api_error)
}

pub(super) async fn detach_plan(
    State(state): State<Arc<AppState>>,
    Path((plan_id, pty_session_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<PlanView>> {
    plans::detach(&state.pool, plan_id, pty_session_id, &PlanActor::user())
        .await
        .map(Json)
        .map_err(plan_api_error)
}

pub(super) async fn branch_plan(
    State(state): State<Arc<AppState>>,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<BranchPlanInput>,
) -> ApiResult<(StatusCode, Json<PlanView>)> {
    let plan = plans::branch(&state.pool, plan_id, request, &PlanActor::user())
        .await
        .map_err(plan_api_error)?;
    Ok((StatusCode::CREATED, Json(plan)))
}

pub(super) async fn plan_tree(
    State(state): State<Arc<AppState>>,
    Path(plan_id): Path<Uuid>,
) -> ApiResult<Json<Vec<PlanTreeNodeView>>> {
    plans::tree(&state.pool, plan_id)
        .await
        .map(Json)
        .map_err(plan_api_error)
}

pub(super) async fn plan_events(
    State(state): State<Arc<AppState>>,
    Path(plan_id): Path<Uuid>,
) -> ApiResult<Json<Vec<PlanEventView>>> {
    plans::events(&state.pool, plan_id)
        .await
        .map(Json)
        .map_err(plan_api_error)
}

fn plan_api_error(error: anyhow::Error) -> ApiError {
    if error.downcast_ref::<sqlx::Error>().is_some() {
        return ApiError::Internal(error);
    }
    let message = error.to_string();
    if message.starts_with("plan not found") || message.starts_with("phase not found") {
        ApiError::NotFound
    } else {
        ApiError::BadRequest(message)
    }
}
