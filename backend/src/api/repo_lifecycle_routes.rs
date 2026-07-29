//! `/api/repos/:name` lifecycle handlers: rename and delete.
//!
//! Both forward to the node that owns the repo. The work itself — the refusal
//! checks, the directory move, and the records that follow it — lives in
//! [`crate::repo_lifecycle`], which the node calls directly.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::node_proxy;
use super::routes::{validate_repo_name, ApiError, ApiResult};
use crate::node_protocol::NodeRequestKind;
use crate::node_runtime::{RepoDeleteRequest, RepoRenameRequest};
use crate::repo_lifecycle::RepoLifecycleError;
use crate::AppState;

impl From<RepoLifecycleError> for ApiError {
    fn from(error: RepoLifecycleError) -> Self {
        match error {
            RepoLifecycleError::NotFound => ApiError::NotFound,
            RepoLifecycleError::BadRequest(message) => ApiError::BadRequest(message),
            RepoLifecycleError::Internal(err) => ApiError::Internal(err),
            RepoLifecycleError::Db(err) => ApiError::Db(err),
            RepoLifecycleError::Io(err) => ApiError::Io(err),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct RepoView {
    name: String,
    path: String,
}

#[derive(Deserialize)]
pub(super) struct PatchRepoReq {
    name: String,
}

#[derive(Deserialize)]
pub(super) struct DeleteRepoQuery {
    #[serde(default)]
    force: Option<bool>,
}

pub(super) async fn patch_repo(
    State(state): State<Arc<AppState>>,
    Path(old_name): Path<String>,
    Json(req): Json<PatchRepoReq>,
) -> ApiResult<Json<RepoView>> {
    validate_repo_name(&old_name)?;
    let new_name = req.name.trim().to_string();
    validate_repo_name(&new_name)?;

    let node_id = node_proxy::repo_node(&state, &old_name).await?;
    let result = node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoRename,
        serde_json::to_value(RepoRenameRequest { old_name, new_name })
            .map_err(anyhow::Error::from)?,
    )
    .await?;
    Ok(Json(
        serde_json::from_value(result).map_err(anyhow::Error::from)?,
    ))
}

pub(super) async fn delete_repo(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<DeleteRepoQuery>,
) -> ApiResult<StatusCode> {
    validate_repo_name(&name)?;

    let node_id = node_proxy::repo_node(&state, &name).await?;
    node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::RepoDelete,
        serde_json::to_value(RepoDeleteRequest {
            name,
            force: q.force.unwrap_or(false),
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
