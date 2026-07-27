use serde_json::Value;
use uuid::Uuid;

use super::routes::{ApiError, ApiResult};
use crate::node_protocol::{NodeProtocolError, NodeRequestKind};
use crate::AppState;

pub async fn default_node(state: &AppState) -> ApiResult<Uuid> {
    state
        .node_control
        .first_available_node()
        .await
        .map_err(map_error)
}

pub async fn session_node(state: &AppState, session_id: Uuid) -> ApiResult<Uuid> {
    let node_id: Option<Uuid> =
        sqlx::query_scalar("SELECT node_id FROM pty_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(ApiError::NotFound)?;
    node_id
        .ok_or_else(|| ApiError::Unavailable("session is owned by the legacy local runtime".into()))
}

pub async fn repo_node(state: &AppState, repo: &str) -> ApiResult<Uuid> {
    let node_id: Option<Uuid> = sqlx::query_scalar("SELECT node_id FROM repos WHERE name = $1")
        .bind(repo)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(ApiError::NotFound)?;
    node_id.ok_or_else(|| ApiError::Unavailable("repo has no development-node owner".into()))
}

pub async fn workspace_node(state: &AppState, workspace_id: Uuid) -> ApiResult<Uuid> {
    let node_id: Option<Uuid> =
        sqlx::query_scalar("SELECT node_id FROM workspaces WHERE id = $1 AND state <> 'deleted'")
            .bind(workspace_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(ApiError::NotFound)?;
    node_id.ok_or_else(|| ApiError::Unavailable("workspace has no development-node owner".into()))
}

pub async fn request(
    state: &AppState,
    node_id: Uuid,
    kind: NodeRequestKind,
    payload: Value,
) -> ApiResult<Value> {
    state
        .node_control
        .request(node_id, kind, payload)
        .await
        .map_err(map_error)
}

pub fn map_error(error: NodeProtocolError) -> ApiError {
    match error {
        NodeProtocolError::NotFound | NodeProtocolError::UnknownNode => ApiError::NotFound,
        NodeProtocolError::Unavailable => {
            ApiError::Unavailable("development node is unavailable".into())
        }
        NodeProtocolError::Remote { code, message: _ } if code == "not_found" => ApiError::NotFound,
        NodeProtocolError::Remote { code, message }
            if matches!(code.as_str(), "bad_request" | "request_failed") =>
        {
            ApiError::BadRequest(message)
        }
        NodeProtocolError::Remote { code, message } => {
            ApiError::Unavailable(format!("development node {code}: {message}"))
        }
        other => ApiError::Internal(anyhow::anyhow!(other)),
    }
}
