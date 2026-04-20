use std::sync::Arc;

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;

use crate::{db, AppState};

mod future_prompt_routes;
mod routes;
mod stats;
mod timeline_routes;
mod ws;

pub use routes::ApiError;
pub use stats::StatsProbe;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/api/stats", get(stats::stats_handler))
        .route("/ws/sessions/:id", get(ws::attach))
        .merge(routes::router())
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    db: &'static str,
}

async fn health(State(state): State<Arc<AppState>>) -> (StatusCode, Json<Health>) {
    match db::ping(&state.pool).await {
        Ok(()) => (
            StatusCode::OK,
            Json(Health {
                status: "ok",
                db: "ok",
            }),
        ),
        Err(err) => {
            tracing::error!(error = %err, "db ping failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(Health {
                    status: "degraded",
                    db: "unreachable",
                }),
            )
        }
    }
}
