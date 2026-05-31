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

#[derive(Deserialize, Serialize)]
pub(super) struct RetrievalReindexRequest {
    repo: Option<String>,
    agent_session_uuid: Option<Uuid>,
    limit: Option<i64>,
    max_batches: Option<u32>,
}

#[derive(Deserialize, Serialize)]
pub(super) struct RetrievalReindexResponse {
    embedded: usize,
    skipped: usize,
    batches: u32,
    complete: bool,
    vector: RetrievalVectorStatus,
    embedding_model: String,
    embedding_dimensions: i32,
}

#[derive(Deserialize)]
struct RetrievalServiceReindexResponse {
    embedded: usize,
    skipped: usize,
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
    let max_batches = request.max_batches.unwrap_or(20).clamp(1, 200);
    let client = reqwest::Client::new();
    let mut total_embedded = 0_usize;
    let mut total_skipped = 0_usize;
    let mut batches = 0_u32;
    let mut last = None;

    for _ in 0..max_batches {
        let batch = call_retrieval_reindex(&client, &target, &request).await?;
        batches += 1;
        total_embedded += batch.embedded;
        total_skipped += batch.skipped;
        let done = batch.embedded == 0;
        last = Some(batch);
        if done {
            break;
        }
    }

    let Some(last) = last else {
        return Err(ApiError::Unavailable(
            "retrieval backfill did not run".to_string(),
        ));
    };
    Ok(Json(RetrievalReindexResponse {
        embedded: total_embedded,
        skipped: total_skipped,
        batches,
        complete: last.embedded == 0,
        vector: last.vector,
        embedding_model: last.embedding_model,
        embedding_dimensions: last.embedding_dimensions,
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
            limit: self.limit.map(|limit| limit.clamp(1, 5000)),
            max_batches: self.max_batches,
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
