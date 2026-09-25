use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    extract::{Path, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
    Extension, Json, Router,
};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    node_proxy,
    routes::{ApiError, ApiResult},
};
use crate::{
    auth::AuthenticatedUser,
    node_protocol::NodeRequestKind,
    uploads::{
        model::{ImportUpload, UploadInput},
        store::StagingStore,
    },
    AppState,
};

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/uploads", post(create))
        .route("/api/uploads/:id/complete", post(complete))
        .layer(middleware::from_fn(bounded_metadata))
}

async fn bounded_metadata(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, 4096).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({"error": "Upload metadata is too large."})),
            )
                .into_response()
        }
    };
    let mut response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

async fn store(state: &AppState) -> ApiResult<&StagingStore> {
    state
        .upload_store
        .get_or_try_init(|| async {
            StagingStore::from_env().await.ok_or_else(|| {
                ApiError::Unavailable("Remote upload storage is not configured.".into())
            })
        })
        .await
}

async fn destination(state: &AppState, input: &UploadInput) -> ApiResult<Uuid> {
    input
        .validate()
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    match (&input.repo, input.workspace_id) {
        (Some(repo), None) => node_proxy::repo_node(state, repo).await,
        (None, Some(id)) => node_proxy::workspace_node(state, id).await,
        _ => unreachable!(),
    }
}

async fn create(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(input): Json<UploadInput>,
) -> ApiResult<Json<Value>> {
    destination(&state, &input).await?;
    let id = Uuid::new_v4();
    let grant = store(&state)
        .await?
        .put_grant(id, input.size, &input.checksum, &input.binding(&user.sub))
        .await
        .map_err(|_| {
            ApiError::Unavailable("Could not authorize file storage. Try again.".into())
        })?;
    Ok(Json(json!({"id": id, "grant": grant})))
}

async fn complete(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<Uuid>,
    Json(input): Json<UploadInput>,
) -> ApiResult<Json<Value>> {
    let node = destination(&state, &input).await?;
    let store = store(&state).await?;
    let valid = store
        .verify(id, input.size, &input.checksum, &input.binding(&user.sub))
        .await
        .map_err(|_| ApiError::Unavailable("Could not verify file storage. Try again.".into()))?;
    if !valid {
        return Err(ApiError::BadRequest(
            "The staged file is missing or does not match this upload.".into(),
        ));
    }
    let url = store.get_grant(id).await.map_err(|_| {
        ApiError::Unavailable("Could not authorize file download. Try again.".into())
    })?;
    let payload = ImportUpload {
        id,
        input,
        url,
        bucket: store.bucket().into(),
        region: store.region().into(),
    };
    let result =
        node_proxy::request(&state, node, NodeRequestKind::UploadImport, json!(payload)).await?;
    Ok(Json(result))
}
