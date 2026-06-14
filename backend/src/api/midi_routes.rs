//! MIDI clip ingest. Authenticated by a device token (see
//! [`super::device_routes`]); the first producer is the Ableton "Send to
//! Sulion" extension. Notes are stored verbatim as JSONB.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::device_routes::DevicePrincipal;
use super::routes::{ApiError, ApiResult};
use crate::AppState;

const MAX_NOTES: usize = 100_000;

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/midi/ingest", post(ingest))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IngestRequest {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tempo: Option<f64>,
    #[serde(default)]
    length_beats: Option<f64>,
    #[serde(default)]
    time_signature: Option<TimeSignature>,
    notes: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct TimeSignature {
    numerator: i32,
    denominator: i32,
}

#[derive(Serialize)]
struct IngestResponse {
    ingest_id: Uuid,
    note_count: i64,
}

async fn ingest(
    State(state): State<Arc<AppState>>,
    Extension(principal): Extension<DevicePrincipal>,
    Json(req): Json<IngestRequest>,
) -> ApiResult<Json<IngestResponse>> {
    if req.notes.is_empty() {
        return Err(ApiError::BadRequest("clip has no notes".into()));
    }
    if req.notes.len() > MAX_NOTES {
        return Err(ApiError::BadRequest(format!(
            "too many notes (max {MAX_NOTES})"
        )));
    }

    let note_count = req.notes.len() as i64;
    let ingest_id = Uuid::new_v4();
    let source = req
        .source
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let (numerator, denominator) = match req.time_signature {
        Some(ts) => (Some(ts.numerator), Some(ts.denominator)),
        None => (None, None),
    };
    let notes = serde_json::Value::Array(req.notes);

    sqlx::query(
        "INSERT INTO midi_clips \
           (ingest_id, device_token_id, user_sub, source, name, tempo, \
            length_beats, time_sig_numerator, time_sig_denominator, note_count, notes) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(ingest_id)
    .bind(principal.token_id)
    .bind(&principal.user_sub)
    .bind(&source)
    .bind(&req.name)
    .bind(req.tempo)
    .bind(req.length_beats)
    .bind(numerator)
    .bind(denominator)
    .bind(note_count as i32)
    .bind(notes)
    .execute(&state.pool)
    .await
    .map_err(ApiError::Db)?;

    tracing::info!(%ingest_id, note_count, source = %source, "midi clip ingested");
    Ok(Json(IngestResponse {
        ingest_id,
        note_count,
    }))
}
