use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::db::{self, Pool};

pub mod api;
pub mod help;
pub mod indexer;
pub mod lsp;
pub mod navigation;
pub mod parser;
pub mod structural;
pub mod symbols;

const DEFAULT_ALLOWED_ROOTS: &str = "/home/dev/repos,/home/dev/workspaces";
const BACKGROUND_INDEX_SECONDS: u64 = 300;

#[derive(Debug, Clone)]
pub struct CodeIntelConfig {
    pub listen: SocketAddr,
    pub db_url: String,
    pub token: String,
    pub allowed_roots: Vec<PathBuf>,
}

impl CodeIntelConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen = std::env::var("SULION_CODE_INTEL_LISTEN")
            .unwrap_or_else(|_| "0.0.0.0:8084".to_string())
            .parse()
            .context("invalid SULION_CODE_INTEL_LISTEN")?;
        let db_url = std::env::var("SULION_DB_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .map_err(|_| anyhow!("SULION_DB_URL or DATABASE_URL must be set"))?;
        let token = env_required("SULION_CODE_INTEL_TOKEN")?;
        let allowed_roots = parse_allowed_roots(
            env_optional("SULION_CODE_INTEL_ALLOWED_ROOTS")
                .as_deref()
                .unwrap_or(DEFAULT_ALLOWED_ROOTS),
        )?;
        Ok(Self {
            listen,
            db_url,
            token,
            allowed_roots,
        })
    }
}

#[derive(Clone)]
pub struct CodeIntelState {
    pub(crate) pool: Pool,
    pub(crate) config: Arc<CodeIntelConfig>,
    pub(crate) lsp: lsp::LspManager,
    pub(crate) index_lock: Arc<Mutex<()>>,
}

impl CodeIntelState {
    pub async fn from_config(config: CodeIntelConfig) -> anyhow::Result<Arc<Self>> {
        let pool = db::connect_and_wait_for_migrations(&config.db_url, "code-intel").await?;
        indexer::cancel_orphaned_running_jobs(&pool).await?;
        let state = Arc::new(Self {
            pool,
            config: Arc::new(config),
            lsp: lsp::LspManager::default(),
            index_lock: Arc::new(Mutex::new(())),
        });
        tokio::spawn(indexer::run_startup_and_background_indexer(
            state.clone(),
            Duration::from_secs(BACKGROUND_INDEX_SECONDS),
        ));
        Ok(state)
    }

    pub fn from_pool_for_tests(pool: Pool, config: CodeIntelConfig) -> Arc<Self> {
        Arc::new(Self {
            pool,
            config: Arc::new(config),
            lsp: lsp::LspManager::default(),
            index_lock: Arc::new(Mutex::new(())),
        })
    }
}

pub fn app(state: Arc<CodeIntelState>) -> Router {
    let protected = api::router().route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        api::require_code_intel_auth,
    ));
    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .with_state(state)
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    database: &'static str,
    auth: &'static str,
    allowed_roots: Vec<String>,
}

async fn health(State(state): State<Arc<CodeIntelState>>) -> Json<HealthResponse> {
    let database = if db::ping(&state.pool).await.is_ok() {
        "ok"
    } else {
        "error"
    };
    Json(HealthResponse {
        status: "ok",
        database,
        auth: if state.config.token.is_empty() {
            "missing"
        } else {
            "configured"
        },
        allowed_roots: state
            .config
            .allowed_roots
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    })
}

fn env_required(key: &str) -> anyhow::Result<String> {
    let value = std::env::var(key).map_err(|_| anyhow!("{key} must be set"))?;
    let value = value.trim().to_string();
    if value.is_empty() {
        anyhow::bail!("{key} must not be empty");
    }
    Ok(value)
}

fn env_optional(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_allowed_roots(value: &str) -> anyhow::Result<Vec<PathBuf>> {
    let roots = value
        .split(',')
        .map(str::trim)
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if roots.is_empty() {
        anyhow::bail!("SULION_CODE_INTEL_ALLOWED_ROOTS must include at least one path");
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_allowed_roots_trims_and_skips_empty_entries() {
        let roots = parse_allowed_roots(" /home/dev/repos, ,/home/dev/workspaces ").unwrap();
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/home/dev/repos"),
                PathBuf::from("/home/dev/workspaces")
            ]
        );
    }

    #[test]
    fn parse_allowed_roots_rejects_empty_values() {
        let err = parse_allowed_roots(" , ").unwrap_err();
        assert!(err.to_string().contains("SULION_CODE_INTEL_ALLOWED_ROOTS"));
    }
}
