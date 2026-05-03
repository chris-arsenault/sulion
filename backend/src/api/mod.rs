use std::sync::Arc;

use axum::{extract::State, http::StatusCode, middleware, routing::get, Json, Router};
use serde::Serialize;

use crate::{db, AppState};

mod admin_routes;
mod app_state_routes;
mod future_prompt_routes;
mod library_routes;
mod repo_routes;
mod routes;
mod session_routes;
mod stats;
mod timeline_routes;
mod workspace_routes;
mod ws;

pub use routes::ApiError;
pub use stats::{run_stats_sampler, sample_stats_once, StatsCache, StatsProbe};

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let protected = Router::new()
        .route("/api/app-state", get(app_state_routes::app_state))
        .route("/ws/sessions/:id", get(ws::attach))
        .merge(routes::router())
        .route_layer(middleware::from_fn_with_state(
            state,
            crate::auth::require_http_auth,
        ));

    Router::new().route("/health", get(health)).merge(protected)
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
