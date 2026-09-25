use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::routes::{ApiError, ApiResult};
use crate::{
    ingest::{
        self,
        turn_stream::{self, Cursor, Record},
    },
    AppState,
};

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/timeline/:session/turns/:turn/stream", get(stream))
        .route("/api/timeline/:session/turns/:turn/operations", get(body))
        .route("/api/timeline/:session/turns/:turn/digest", get(digest))
        .route("/api/timeline/:session/summaries", get(summaries))
}

#[derive(Deserialize)]
struct StreamQuery {
    cursor: Option<String>,
}

async fn stream(
    State(state): State<Arc<AppState>>,
    Path((session, turn)): Path<(Uuid, i64)>,
    Query(query): Query<StreamQuery>,
) -> ApiResult<Response> {
    let cursor = query
        .cursor
        .map(|raw| serde_json::from_str::<Cursor>(&raw))
        .transpose()
        .map_err(|_| ApiError::BadRequest("invalid turn cursor".into()))?;
    let header = turn_stream::begin(&state.pool, session, turn, cursor).await?;
    let Record::Header { cursor, .. } = &header else {
        unreachable!()
    };
    let mut cursor = cursor.clone();
    let body = async_stream::stream! {
        yield encode(&header);
        loop {
            let record = match turn_stream::next(&state.pool, session, turn, cursor.clone()).await {
                Ok(record) => record,
                Err(error) => {
                    tracing::warn!(%error, %session, turn, "turn stream failed");
                    yield encode(&Record::Error { message: "Turn loading failed; retry to resume.".into() });
                    break;
                }
            };
            let done = match &record {
                Record::Batch { cursor: next, .. } => { cursor = next.clone(); false }
                _ => true,
            };
            yield encode(&record);
            if done { break; }
        }
    };
    Ok((
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-store"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        Body::from_stream(body),
    )
        .into_response())
}

fn encode(record: &Record) -> Result<bytes::Bytes, std::io::Error> {
    let mut bytes = serde_json::to_vec(record)?;
    bytes.push(b'\n');
    Ok(bytes.into())
}

#[derive(Deserialize)]
struct BodiesQuery {
    ids: String,
}

async fn body(
    State(state): State<Arc<AppState>>,
    Path((session, turn)): Path<(Uuid, i64)>,
    Query(query): Query<BodiesQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    let ids: Vec<String> = serde_json::from_str(&query.ids)
        .map_err(|_| ApiError::BadRequest("invalid operation ids".into()))?;
    if ids.is_empty() || ids.len() > 16 {
        return Err(ApiError::BadRequest(
            "request one to sixteen operation bodies".into(),
        ));
    }
    Ok(Json(
        turn_stream::operation_bodies(&state.pool, session, turn, &ids).await?,
    ))
}

async fn digest(
    State(state): State<Arc<AppState>>,
    Path((session, turn)): Path<(Uuid, i64)>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        serde_json::json!({ "markdown": turn_stream::digest(&state.pool, session, turn).await? }),
    ))
}

async fn summaries(
    State(state): State<Arc<AppState>>,
    Path(session): Path<Uuid>,
) -> ApiResult<Json<ingest::TimelineSummaryResponse>> {
    let filters = ingest::ProjectionFilters {
        show_sidechain: true,
        ..Default::default()
    };
    let mut response = ingest::turn_stream::summaries(&state.pool, session, &filters).await?;
    let meta = ingest::load_timeline_session_meta(&state.pool, session).await?;
    ingest::annotate_timeline_summaries(&mut response.turns, &meta);
    response.session_uuid = Some(session);
    response.archived_at = meta.archived_at;
    Ok(Json(response))
}
