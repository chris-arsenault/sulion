use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use uuid::Uuid;

use super::routes::{ApiError, ApiResult};
use crate::meta_repos::{
    CreateMetaRepoInput, MetaRepoError, MetaRepoView, PatchMetaRepoInput, ReplaceMembersInput,
};
use crate::AppState;

impl From<MetaRepoError> for ApiError {
    fn from(error: MetaRepoError) -> Self {
        match error {
            MetaRepoError::NotFound => ApiError::NotFound,
            MetaRepoError::BadRequest(message) => ApiError::BadRequest(message),
            MetaRepoError::Db(error) => ApiError::Db(error),
        }
    }
}

pub(super) async fn create_meta_repo(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CreateMetaRepoInput>,
) -> ApiResult<(StatusCode, Json<MetaRepoView>)> {
    let created = crate::meta_repos::create(&state.pool, input).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

pub(super) async fn patch_meta_repo(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(input): Json<PatchMetaRepoInput>,
) -> ApiResult<Json<MetaRepoView>> {
    Ok(Json(
        crate::meta_repos::patch(&state.pool, id, input).await?,
    ))
}

pub(super) async fn replace_meta_repo_members(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(input): Json<ReplaceMembersInput>,
) -> ApiResult<Json<MetaRepoView>> {
    Ok(Json(
        crate::meta_repos::replace_members(&state.pool, id, input).await?,
    ))
}

pub(super) async fn delete_meta_repo(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    crate::meta_repos::delete(&state.pool, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
