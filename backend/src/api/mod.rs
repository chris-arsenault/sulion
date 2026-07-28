use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;

use crate::{db, AppState};

mod admin_routes;
mod app_state_routes;
mod device_routes;
pub(crate) mod file_content;
mod future_prompt_routes;
mod library_routes;
mod node_proxy;
mod plan_routes;
pub(crate) mod repo_lifecycle_routes;
mod repo_routes;
mod routes;
mod session_launch;
mod session_routes;
mod stats;
mod timeline_routes;
mod workspace_routes;
mod ws;

pub use routes::ApiError;
pub use stats::{run_stats_sampler, sample_stats_once, StatsCache, StatsProbe};
pub use ws::WsTicketStore;

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Cognito-authenticated surface. Device-pairing *approval* lives here so the
    // approving user's identity is captured.
    let protected = Router::new()
        .route("/api/app-state", get(app_state_routes::app_state))
        .route("/api/ws-tickets", post(ws::issue_ticket))
        .merge(routes::router())
        .merge(crate::node_protocol::admin_router())
        .merge(device_routes::approve_router())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_http_auth,
        ));

    // Public pairing endpoints — the device has no credential yet.
    let device_public = device_routes::public_router();

    // Device-token-authenticated surface: external tools write content into a
    // repo on disk. Guarded by its own layer (device tokens, not Cognito JWTs).
    let device_authed = Router::new()
        .route(
            "/api/repos/:name/ingest",
            post(repo_routes::post_repo_ingest),
        )
        .route("/api/repos/:name/raw", get(repo_routes::get_repo_raw))
        .route_layer(middleware::from_fn_with_state(
            state,
            device_routes::require_device_token,
        ));

    Router::new()
        .route("/health", get(health))
        .route("/ws/sessions/:id", get(ws::attach))
        .merge(crate::node_protocol::public_router())
        .merge(protected)
        .merge(device_public)
        .merge(device_authed)
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    db: &'static str,
    role: &'static str,
    development_node: &'static str,
}

async fn health(State(state): State<Arc<AppState>>) -> (StatusCode, Json<Health>) {
    let role = if state.node_protocol_required {
        "control-plane"
    } else {
        "standalone"
    };
    let development_node = if !state.node_protocol_required {
        "local"
    } else if state.node_control.active_connection_count().await > 0 {
        "connected"
    } else {
        "unavailable"
    };
    match db::ping(&state.pool).await {
        Ok(()) => (
            StatusCode::OK,
            Json(Health {
                status: "ok",
                db: "ok",
                role,
                development_node,
            }),
        ),
        Err(err) => {
            tracing::error!(error = %err, "db ping failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(Health {
                    status: "degraded",
                    db: "unreachable",
                    role,
                    development_node,
                }),
            )
        }
    }
}
