use std::sync::Arc;

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;

pub mod config;
pub mod db;

#[derive(Clone)]
pub struct AppState {
    pub pool: db::Pool,
}

impl AppState {
    pub fn new(pool: db::Pool) -> Arc<Self> {
        Arc::new(Self { pool })
    }
}

pub fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .with_state(state)
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

/// Verifies that the embedded migration set parses and is visible to the
/// compile-time migrator. Does not require a running database.
pub fn embedded_migrations_present() -> bool {
    !sqlx::migrate!("./migrations").migrations.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_embedded_at_compile_time() {
        assert!(embedded_migrations_present());
    }
}
