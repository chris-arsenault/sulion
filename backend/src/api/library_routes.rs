use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use super::routes::{ApiError, ApiResult};
use crate::library::{self, LibraryKind};
use crate::AppState;

fn parse_kind(s: &str) -> ApiResult<LibraryKind> {
    LibraryKind::parse(s).ok_or_else(|| ApiError::BadRequest(format!("unknown library kind: {s}")))
}

pub(super) async fn list_library(
    State(state): State<Arc<AppState>>,
    Path(kind): Path<String>,
) -> ApiResult<Json<Vec<library::LibraryEntry>>> {
    let kind = parse_kind(&kind)?;
    let entries = library::list(&state.library_root, kind)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    Ok(Json(entries))
}

pub(super) async fn get_library_entry(
    State(state): State<Arc<AppState>>,
    Path((kind, slug)): Path<(String, String)>,
) -> ApiResult<Json<library::LibraryEntry>> {
    let kind = parse_kind(&kind)?;
    let entry = library::read(&state.library_root, kind, &slug)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(entry))
}

/// POST/PUT to the root URL: derive a slug from the name. Returns the
/// saved entry (including the derived slug) so the client can round-trip.
pub(super) async fn put_library_root(
    State(state): State<Arc<AppState>>,
    Path(kind): Path<String>,
    Json(input): Json<library::SaveInput>,
) -> ApiResult<Json<library::LibraryEntry>> {
    let kind = parse_kind(&kind)?;
    let desired_slug = library::sanitise_slug(&input.name)
        .ok_or_else(|| ApiError::BadRequest("name must derive a valid slug".into()))?;
    let slug = library::next_available_slug(&state.library_root, kind, &desired_slug);
    let entry = library::save(&state.library_root, kind, &slug, input)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    Ok(Json(entry))
}

pub(super) async fn put_library_entry(
    State(state): State<Arc<AppState>>,
    Path((kind, slug)): Path<(String, String)>,
    Json(input): Json<library::SaveInput>,
) -> ApiResult<Json<library::LibraryEntry>> {
    let kind = parse_kind(&kind)?;
    let entry = library::save(&state.library_root, kind, &slug, input)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    Ok(Json(entry))
}

pub(super) async fn delete_library_entry(
    State(state): State<Arc<AppState>>,
    Path((kind, slug)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let kind = parse_kind(&kind)?;
    let removed = library::delete(&state.library_root, kind, &slug)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    Ok(if removed {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    })
}
