//! Admin actions. Destructive or support-tier endpoints that don't
//! belong next to the ordinary session/repo surface.

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

/// `POST /api/admin/archive/run`: queue an archive cycle for the control
/// process's loop. The loop, not this handler, does the work.
#[derive(Deserialize, Default)]
pub(super) struct ArchiveRunRequest {
    #[serde(default)]
    dry_run: bool,
}

pub(super) async fn archive_run(
    State(state): State<Arc<AppState>>,
    body: Option<Json<ArchiveRunRequest>>,
) -> ApiResult<Json<crate::archive::ArchiveRequest>> {
    let request = body.map(|Json(body)| body).unwrap_or_default();
    let queued = crate::archive::requests::enqueue_run(&state.pool, request.dry_run, Some("ui"))
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(queued))
}

/// `POST /api/admin/archive/restore`: queue a restore of purged sessions.
pub(super) async fn archive_restore(
    State(state): State<Arc<AppState>>,
    Json(scope): Json<crate::archive::RestoreScope>,
) -> ApiResult<Json<crate::archive::ArchiveRequest>> {
    if scope.is_empty() {
        return Err(ApiError::BadRequest(
            "restore needs a session_uuid, month, repo, or all".into(),
        ));
    }
    let queued = crate::archive::requests::enqueue_restore(&state.pool, &scope, Some("ui"))
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(queued))
}

/// `POST /api/admin/archive/verify`: queue a check of every archived object.
#[derive(Deserialize, Default)]
pub(super) struct ArchiveVerifyRequest {
    #[serde(default)]
    deep: bool,
}

pub(super) async fn archive_verify(
    State(state): State<Arc<AppState>>,
    body: Option<Json<ArchiveVerifyRequest>>,
) -> ApiResult<Json<crate::archive::ArchiveRequest>> {
    let request = body.map(|Json(body)| body).unwrap_or_default();
    let queued = crate::archive::requests::enqueue_verify(&state.pool, request.deep, Some("ui"))
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(queued))
}

/// `POST /api/admin/archive/purge-gate`: open or close deletion.
#[derive(Deserialize)]
pub(super) struct ArchivePurgeGateRequest {
    enabled: bool,
}

pub(super) async fn archive_purge_gate(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ArchivePurgeGateRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    crate::archive::set_purge_enabled(&state.pool, request.enabled)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(serde_json::json!({ "purge_enabled": request.enabled })))
}

/// `GET /api/admin/archive`: store, last cycle, counts, recent requests.
pub(super) async fn archive_status(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<crate::archive::ArchiveStatus>> {
    let store = crate::archive::ObjectStore::from_env();
    let status = crate::archive::status(&state.pool, store.as_ref())
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(status))
}

#[derive(Deserialize, Serialize)]
pub(super) struct RetrievalReindexRequest {
    repo: Option<String>,
    agent_session_uuid: Option<Uuid>,
}

#[derive(Deserialize, Serialize)]
pub(super) struct RetrievalReindexResponse {
    generation: i64,
    backfills_started: usize,
    sources_seen: usize,
    sources_marked_pending: usize,
    sources_deleted: usize,
    pending_sources: i64,
    vector: RetrievalVectorStatus,
    embedding_model: String,
    embedding_dimensions: i32,
}

#[derive(Deserialize)]
struct RetrievalServiceReindexResponse {
    generation: i64,
    backfills_started: usize,
    sources_seen: usize,
    sources_marked_pending: usize,
    sources_deleted: usize,
    pending_sources: i64,
    vector: RetrievalVectorStatus,
    embedding_model: String,
    embedding_dimensions: i32,
}

#[derive(Deserialize, Serialize)]
pub(super) struct RetrievalVectorStatus {
    extension_installed: bool,
    column_exists: bool,
    ann_index_exists: bool,
}

pub(super) async fn retrieval_reindex(
    Json(request): Json<RetrievalReindexRequest>,
) -> ApiResult<Json<RetrievalReindexResponse>> {
    let target = RetrievalServiceTarget::from_env()?;
    let request = request.normalized();
    let client = reqwest::Client::new();
    let marked = call_retrieval_reindex(&client, &target, &request).await?;
    Ok(Json(RetrievalReindexResponse {
        generation: marked.generation,
        backfills_started: marked.backfills_started,
        sources_seen: marked.sources_seen,
        sources_marked_pending: marked.sources_marked_pending,
        sources_deleted: marked.sources_deleted,
        pending_sources: marked.pending_sources,
        vector: marked.vector,
        embedding_model: marked.embedding_model,
        embedding_dimensions: marked.embedding_dimensions,
    }))
}

impl RetrievalReindexRequest {
    fn normalized(&self) -> Self {
        Self {
            repo: self
                .repo
                .as_ref()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            agent_session_uuid: self.agent_session_uuid,
        }
    }
}

async fn call_retrieval_reindex(
    client: &reqwest::Client,
    target: &RetrievalServiceTarget,
    request: &RetrievalReindexRequest,
) -> ApiResult<RetrievalServiceReindexResponse> {
    let response = client
        .post(target.url("/v1/reindex"))
        .bearer_auth(&target.token)
        .json(request)
        .send()
        .await
        .map_err(|err| ApiError::Unavailable(format!("retrieval service request failed: {err}")))?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ApiError::Unavailable(format!(
            "retrieval service rejected backfill ({status}): {body}"
        )));
    }
    serde_json::from_str::<RetrievalServiceReindexResponse>(&body)
        .map_err(|err| ApiError::Unavailable(format!("invalid retrieval response: {err}")))
}

struct RetrievalServiceTarget {
    base_url: String,
    token: String,
}

impl RetrievalServiceTarget {
    fn from_env() -> ApiResult<Self> {
        let base_url = env_required("SULION_RETRIEVAL_URL")?;
        let token = env_required("SULION_RETRIEVAL_TOKEN")?;
        Ok(Self { base_url, token })
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

fn env_required(key: &str) -> ApiResult<String> {
    std::env::var(key)
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::Unavailable(format!("{key} is not configured")))
}
