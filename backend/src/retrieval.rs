use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use ring::digest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::db::{self, Pool};

mod context;
mod embeddings;
mod routes_extra;
mod search;

use context::{
    clean_opt, context_route, infer_repo_from_cwd, resolve_context, ContextQuery, ResolvedContext,
};
use embeddings::{
    bootstrap_index_if_empty, index_has_work, index_status_route, reindex_route, reset_route,
    run_retrieval_indexer_once, EmbeddingClient,
};
use routes_extra::{facets_route, file_history_route, sessions_route, turn_route};
use search::{load_evidence, search_route, EvidencePacket};

const DEFAULT_EMBEDDING_SERVICE_URL: &str = "http://192.168.66.3:5361";
const DEFAULT_EMBEDDING_MODEL: &str = "nomic-ai/nomic-embed-text-v1.5";
const DEFAULT_EMBEDDING_DIMENSIONS: i32 = 768;
const DEFAULT_BATCH_SIZE: usize = 64;
const DEFAULT_EMBEDDING_MAX_CHARS: usize = 6000;
/// Max chunks a single source can produce. Long prose (assistant/user/summary
/// text and subagent finals) is split into `embedding_max_chars`-sized chunks up
/// to this many; the tail beyond it is dropped so one pathological source can't
/// flood the index.
const DEFAULT_EMBEDDING_CHUNK_MAX: usize = 10;
/// Minimum cosine similarity (0..1) a semantic hit must clear to be returned.
/// Below this we'd be surfacing the nearest neighbor regardless of relevance, so
/// semantic mode returns nothing rather than noise. Tune via env.
const DEFAULT_SEMANTIC_MIN_SCORE: f32 = 0.5;
const MAX_BATCH_SIZE: usize = 256;
const MAX_EMBEDDING_MAX_CHARS: usize = 20000;
const DEFAULT_REINDEX_LIMIT: i64 = 500;
const BACKGROUND_INDEX_SECONDS: u64 = 300;
const BACKGROUND_BUSY_DELAY: Duration = Duration::from_secs(1);
const MAX_SEARCH_LIMIT: i64 = 100;
const MAX_REINDEX_LIMIT: i64 = 5000;

#[derive(Debug, Clone)]
pub struct RetrievalConfig {
    pub listen: SocketAddr,
    pub db_url: String,
    pub token: String,
    pub embedding_service_url: String,
    pub embedding_model: String,
    pub embedding_dimensions: i32,
    pub embedding_batch_size: usize,
    pub embedding_max_chars: usize,
    pub embedding_chunk_max: usize,
    pub semantic_min_score: f32,
    pub background_index_seconds: Option<u64>,
}

impl RetrievalConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen = std::env::var("SULION_RETRIEVAL_LISTEN")
            .unwrap_or_else(|_| "0.0.0.0:8083".to_string())
            .parse()
            .context("invalid SULION_RETRIEVAL_LISTEN")?;
        let db_url =
            std::env::var("SULION_DB_URL").map_err(|_| anyhow!("SULION_DB_URL must be set"))?;
        let token = env_required("SULION_RETRIEVAL_TOKEN")?;
        let embedding_service_url = env_optional("SULION_RETRIEVAL_EMBEDDING_URL")
            .unwrap_or_else(|| DEFAULT_EMBEDDING_SERVICE_URL.to_string());
        let embedding_model = env_optional("SULION_RETRIEVAL_EMBEDDING_MODEL")
            .unwrap_or_else(|| DEFAULT_EMBEDDING_MODEL.to_string());
        let embedding_dimensions = env_optional("SULION_RETRIEVAL_EMBEDDING_DIMENSIONS")
            .map(|value| value.parse())
            .transpose()
            .context("invalid SULION_RETRIEVAL_EMBEDDING_DIMENSIONS")?
            .unwrap_or(DEFAULT_EMBEDDING_DIMENSIONS);
        if embedding_dimensions <= 0 {
            anyhow::bail!("SULION_RETRIEVAL_EMBEDDING_DIMENSIONS must be positive");
        }
        let embedding_batch_size = env_optional("SULION_RETRIEVAL_EMBEDDING_BATCH_SIZE")
            .map(|value| value.parse::<usize>())
            .transpose()
            .context("invalid SULION_RETRIEVAL_EMBEDDING_BATCH_SIZE")?
            .unwrap_or(DEFAULT_BATCH_SIZE)
            .clamp(1, MAX_BATCH_SIZE);
        let embedding_max_chars = env_optional("SULION_RETRIEVAL_EMBEDDING_MAX_CHARS")
            .map(|value| value.parse::<usize>())
            .transpose()
            .context("invalid SULION_RETRIEVAL_EMBEDDING_MAX_CHARS")?
            .unwrap_or(DEFAULT_EMBEDDING_MAX_CHARS)
            .clamp(256, MAX_EMBEDDING_MAX_CHARS);
        let embedding_chunk_max = env_optional("SULION_RETRIEVAL_EMBEDDING_CHUNK_MAX")
            .map(|value| value.parse::<usize>())
            .transpose()
            .context("invalid SULION_RETRIEVAL_EMBEDDING_CHUNK_MAX")?
            .unwrap_or(DEFAULT_EMBEDDING_CHUNK_MAX)
            .max(1);
        let semantic_min_score = env_optional("SULION_RETRIEVAL_SEMANTIC_MIN_SCORE")
            .map(|value| value.parse::<f32>())
            .transpose()
            .context("invalid SULION_RETRIEVAL_SEMANTIC_MIN_SCORE")?
            .unwrap_or(DEFAULT_SEMANTIC_MIN_SCORE)
            .clamp(0.0, 1.0);
        let background_index_seconds = env_optional("SULION_RETRIEVAL_INDEX_SECONDS")
            .or_else(|| env_optional("SULION_RETRIEVAL_REINDEX_SECONDS"))
            .map(|value| value.parse::<u64>())
            .transpose()
            .context("invalid SULION_RETRIEVAL_INDEX_SECONDS")?
            .or(Some(BACKGROUND_INDEX_SECONDS))
            // `0` disables the background indexer entirely: no worker is spawned
            // (see the `if let Some(seconds)` guard at startup). Unset falls back
            // to the default interval; any positive value is the loop cadence.
            .filter(|seconds| *seconds > 0);
        Ok(Self {
            listen,
            db_url,
            token,
            embedding_service_url,
            embedding_model,
            embedding_dimensions,
            embedding_batch_size,
            embedding_max_chars,
            embedding_chunk_max,
            semantic_min_score,
            background_index_seconds,
        })
    }
}

fn env_required(key: &str) -> anyhow::Result<String> {
    let value = std::env::var(key).map_err(|_| anyhow!("{key} must be set"))?;
    let value = value.trim().to_string();
    if value.is_empty() {
        anyhow::bail!("{key} must not be empty");
    }
    Ok(value)
}

use crate::config::env_optional;

#[derive(Clone)]
pub struct RetrievalState {
    pool: Pool,
    config: Arc<RetrievalConfig>,
    http: reqwest::Client,
    vector_capabilities: Arc<RwLock<VectorCapabilities>>,
    index_lock: Arc<Mutex<()>>,
}

impl RetrievalState {
    pub async fn from_config(config: RetrievalConfig) -> anyhow::Result<Arc<Self>> {
        let pool = db::connect_and_wait_for_migrations(&config.db_url, "retrieval").await?;
        let state = Arc::new(Self {
            pool,
            config: Arc::new(config),
            http: reqwest::Client::new(),
            vector_capabilities: Arc::new(RwLock::new(VectorCapabilities::default())),
            index_lock: Arc::new(Mutex::new(())),
        });
        state.initialize_vector_capabilities().await?;
        bootstrap_index_if_empty(&state)
            .await
            .map_err(|err| anyhow!(err.to_string()))?;
        if let Some(seconds) = state.config.background_index_seconds {
            tokio::spawn(run_background_indexer(state.clone(), seconds));
        }
        Ok(state)
    }

    pub fn from_pool_for_tests(pool: Pool, token: impl Into<String>) -> Arc<Self> {
        Self::from_pool_with_config_for_tests(
            pool,
            RetrievalConfig {
                listen: "127.0.0.1:0".parse().expect("listen"),
                db_url: String::new(),
                token: token.into(),
                embedding_service_url: DEFAULT_EMBEDDING_SERVICE_URL.to_string(),
                embedding_model: DEFAULT_EMBEDDING_MODEL.to_string(),
                embedding_dimensions: DEFAULT_EMBEDDING_DIMENSIONS,
                embedding_batch_size: DEFAULT_BATCH_SIZE,
                embedding_max_chars: DEFAULT_EMBEDDING_MAX_CHARS,
                embedding_chunk_max: DEFAULT_EMBEDDING_CHUNK_MAX,
                semantic_min_score: DEFAULT_SEMANTIC_MIN_SCORE,
                background_index_seconds: None,
            },
        )
    }

    pub fn from_pool_with_config_for_tests(pool: Pool, config: RetrievalConfig) -> Arc<Self> {
        Arc::new(Self {
            pool,
            config: Arc::new(config),
            http: reqwest::Client::new(),
            vector_capabilities: Arc::new(RwLock::new(VectorCapabilities::default())),
            index_lock: Arc::new(Mutex::new(())),
        })
    }

    async fn initialize_vector_capabilities(&self) -> anyhow::Result<VectorCapabilities> {
        let extension_installed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')",
        )
        .fetch_one(&self.pool)
        .await
        .context("check pgvector extension")?;

        let mut column_exists = false;
        let mut ann_index_exists = false;
        if extension_installed {
            let dimensions = self.config.embedding_dimensions;
            let expected_type = format!("vector({dimensions})");
            let mut column_type: Option<String> = sqlx::query_scalar(
                "SELECT format_type(a.atttypid, a.atttypmod) \
                   FROM pg_attribute a \
                   JOIN pg_class c ON c.oid = a.attrelid \
                   JOIN pg_namespace n ON n.oid = c.relnamespace \
                  WHERE n.nspname = 'public' \
                    AND c.relname = 'retrieval_embeddings' \
                    AND a.attname = 'embedding_vector' \
                    AND NOT a.attisdropped",
            )
            .fetch_optional(&self.pool)
            .await
            .context("check embedding_vector column")?;

            if column_type.is_none() {
                let add_column_sql = format!(
                    "ALTER TABLE retrieval_embeddings \
                     ADD COLUMN IF NOT EXISTS embedding_vector vector({dimensions})"
                );
                sqlx::query(&add_column_sql)
                    .execute(&self.pool)
                    .await
                    .context("add retrieval embedding_vector column")?;
                column_type = Some(expected_type.clone());
            }

            column_exists = column_type.as_deref() == Some(expected_type.as_str());
            if let Some(column_type) = column_type {
                if column_type != expected_type {
                    tracing::warn!(
                        actual = %column_type,
                        expected = %expected_type,
                        "retrieval embedding_vector column has incompatible pgvector dimension; semantic search will use exact REAL[] scan"
                    );
                }
            }

            if column_exists {
                let index_name = format!("retrieval_embeddings_embedding_hnsw_{dimensions}_idx");
                ann_index_exists = self.pgvector_index_ready(&index_name).await?;

                if !ann_index_exists {
                    // CONCURRENTLY must run outside a transaction. This path is
                    // startup-only and reached only when the catalog says the
                    // configured ANN index is absent or unusable.
                    let index_sql = format!(
                        "CREATE INDEX CONCURRENTLY IF NOT EXISTS {index_name} \
                         ON retrieval_embeddings USING hnsw (embedding_vector vector_cosine_ops) \
                         WHERE embedding_dimensions = {dimensions}",
                    );
                    sqlx::query(&index_sql)
                        .execute(&self.pool)
                        .await
                        .context("create retrieval pgvector HNSW index")?;
                    ann_index_exists = self.pgvector_index_ready(&index_name).await?;
                    if !ann_index_exists {
                        tracing::warn!(
                            index = %index_name,
                            "retrieval pgvector HNSW index is not ready; semantic search will use exact REAL[] scan"
                        );
                    }
                }
            }
        }

        let capabilities = VectorCapabilities {
            extension_installed,
            column_exists,
            ann_index_exists,
        };
        *self.vector_capabilities.write().await = capabilities;
        Ok(capabilities)
    }

    async fn pgvector_index_ready(&self, index_name: &str) -> anyhow::Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS ( \
                 SELECT 1 \
                   FROM pg_class idx \
                   JOIN pg_namespace n ON n.oid = idx.relnamespace \
                   JOIN pg_index i ON i.indexrelid = idx.oid \
                  WHERE n.nspname = 'public' \
                    AND idx.relname = $1 \
                    AND i.indisvalid \
                    AND i.indisready \
             )",
        )
        .bind(index_name)
        .fetch_one(&self.pool)
        .await
        .context("check retrieval pgvector HNSW index")
    }

    fn embedding_client(&self) -> EmbeddingClient {
        EmbeddingClient {
            http: self.http.clone(),
            service_url: self.config.embedding_service_url.clone(),
            model: self.config.embedding_model.clone(),
            dimensions: self.config.embedding_dimensions,
        }
    }
}

#[cfg(feature = "integration-tests")]
pub async fn run_indexer_once_for_tests(
    state: &Arc<RetrievalState>,
    limit: i64,
) -> anyhow::Result<()> {
    run_retrieval_indexer_once(state, limit)
        .await
        .map(|_| ())
        .map_err(|err| anyhow!(err.to_string()))
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct VectorCapabilities {
    pub extension_installed: bool,
    pub column_exists: bool,
    pub ann_index_exists: bool,
}

async fn run_background_indexer(state: Arc<RetrievalState>, seconds: u64) {
    let idle_delay = Duration::from_secs(seconds.max(5));
    loop {
        let delay = match run_retrieval_indexer_once(&state, DEFAULT_REINDEX_LIMIT).await {
            Ok(_) => match index_has_work(&state).await {
                Ok(true) => BACKGROUND_BUSY_DELAY,
                Ok(false) => idle_delay,
                Err(err) => {
                    tracing::warn!(%err, "retrieval background index status check failed");
                    idle_delay
                }
            },
            Err(err) => {
                tracing::warn!(%err, "retrieval background indexer failed");
                idle_delay
            }
        };
        tokio::time::sleep(delay).await;
    }
}

pub fn app(state: Arc<RetrievalState>) -> Router {
    let protected = Router::new()
        .route("/v1/context", get(context_route))
        .route("/v1/search", get(search_route))
        .route("/v1/sessions", get(sessions_route))
        .route("/v1/files/history", get(file_history_route))
        .route("/v1/facets", get(facets_route))
        .route("/v1/turns", get(turn_route))
        .route("/v1/reindex", post(reindex_route))
        .route("/v1/index/status", get(index_status_route))
        .route("/v1/index/reset", post(reset_route))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_retrieval_auth,
        ));

    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .with_state(state)
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    vector: VectorCapabilities,
    embedding_service_url: String,
    embedding_model: String,
    embedding_dimensions: i32,
}

async fn health(State(state): State<Arc<RetrievalState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        vector: *state.vector_capabilities.read().await,
        embedding_service_url: state.config.embedding_service_url.clone(),
        embedding_model: state.config.embedding_model.clone(),
        embedding_dimensions: state.config.embedding_dimensions,
    })
}

#[derive(Debug, Deserialize)]
struct AccessTokenQuery {
    access_token: Option<String>,
}

async fn require_retrieval_auth(
    State(state): State<Arc<RetrievalState>>,
    query: Query<AccessTokenQuery>,
    req: Request,
    next: Next,
) -> Response {
    let token = bearer_from_request(req.headers(), query.access_token.as_deref());
    if token != Some(state.config.token.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    next.run(req).await
}

fn bearer_from_request<'a>(
    headers: &'a HeaderMap,
    query_token: Option<&'a str>,
) -> Option<&'a str> {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        if let Ok(value) = value.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                if !token.trim().is_empty() {
                    return Some(token);
                }
            }
        }
    }
    if let Some(value) = headers.get("x-sulion-retrieval-token") {
        if let Ok(value) = value.to_str() {
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
    }
    query_token.filter(|token| !token.trim().is_empty())
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug)]
struct RetrievalError {
    status: StatusCode,
    message: String,
}

impl RetrievalError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
        }
    }

    fn internal(err: anyhow::Error) -> Self {
        tracing::error!(%err, "retrieval internal error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal error".to_string(),
        }
    }
}

impl IntoResponse for RetrievalError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

impl std::fmt::Display for RetrievalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status, self.message)
    }
}

impl From<sqlx::Error> for RetrievalError {
    fn from(err: sqlx::Error) -> Self {
        Self::internal(err.into())
    }
}

impl From<anyhow::Error> for RetrievalError {
    fn from(err: anyhow::Error) -> Self {
        Self::internal(err)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Repo,
    Session,
    All,
}

impl Scope {
    fn parse(raw: Option<&str>) -> Result<Self, RetrievalError> {
        match raw.unwrap_or("repo") {
            "" | "repo" => Ok(Self::Repo),
            "session" => Ok(Self::Session),
            "all" => Ok(Self::All),
            other => Err(RetrievalError::bad_request(format!(
                "unsupported scope: {other}"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Repo => "repo",
            Self::Session => "session",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchMode {
    Hybrid,
    Lexical,
    Semantic,
}

impl SearchMode {
    fn parse(raw: Option<&str>) -> Result<Self, RetrievalError> {
        match raw.unwrap_or("hybrid") {
            "" | "hybrid" => Ok(Self::Hybrid),
            "lexical" => Ok(Self::Lexical),
            "semantic" => Ok(Self::Semantic),
            other => Err(RetrievalError::bad_request(format!(
                "unsupported search_mode: {other}"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Hybrid => "hybrid",
            Self::Lexical => "lexical",
            Self::Semantic => "semantic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
enum SourceKind {
    AssistantText,
    UserPrompt,
    Summary,
    ToolCall,
    ToolResult,
    ToolError,
    TurnDigest,
}

impl SourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::AssistantText => "assistant_text",
            Self::UserPrompt => "user_prompt",
            Self::Summary => "summary",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::ToolError => "tool_error",
            Self::TurnDigest => "turn_digest",
        }
    }

    fn is_tool(self) -> bool {
        matches!(self, Self::ToolCall | Self::ToolResult | Self::ToolError)
    }
}

fn parse_includes(raw: Option<&str>) -> Result<HashSet<SourceKind>, RetrievalError> {
    let mut out = HashSet::new();
    let raw = raw.unwrap_or("assistant");
    for item in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let kind = match item {
            "assistant" | "assistant_text" => SourceKind::AssistantText,
            "user" | "user_prompt" => SourceKind::UserPrompt,
            "summary" => SourceKind::Summary,
            "tool" | "tools" => {
                out.insert(SourceKind::ToolCall);
                out.insert(SourceKind::ToolResult);
                out.insert(SourceKind::ToolError);
                continue;
            }
            "tool_call" | "tool_input" => SourceKind::ToolCall,
            "tool_result" => SourceKind::ToolResult,
            "tool_error" => SourceKind::ToolError,
            "turn_digest" => SourceKind::TurnDigest,
            other => {
                return Err(RetrievalError::bad_request(format!(
                    "unsupported include: {other}"
                )));
            }
        };
        out.insert(kind);
    }
    if out.is_empty() {
        return Err(RetrievalError::bad_request("include must not be empty"));
    }
    Ok(out)
}

fn vector_literal(values: &[f32]) -> String {
    let mut out = String::with_capacity(values.len() * 12);
    out.push('[');
    for (idx, value) in values.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&value.to_string());
    }
    out.push(']');
    out
}
