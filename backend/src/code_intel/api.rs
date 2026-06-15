use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

mod model;
mod nav;
mod pack;
mod root;

use model::{
    confidence_for_summary, escape_like, freshness_for_summary, rows_to_symbol_results,
    summary_warnings, Budget, CommandResponse, IndexJobView, IndexStatusResponse, IndexSummary,
    IndexSummaryView, PatchResponse, RefreshResponse, RefreshStatsView, RootView, SemanticStatus,
    StatusResponse, SymbolResult,
};
use root::{resolve_target, ResolvedTarget, TargetKind};

use super::indexer::{self, CodeRootSpec, IndexOptions, IndexTrigger, RefreshStats};
use super::parser::{SourceLanguage, SourceWalkOptions};
use super::structural::{self, StructuralLanguage, StructuralMatchResult};
use super::CodeIntelState;
use crate::code_intel::help::{help_response, HelpResponse};
use crate::db::Pool;

const SCHEMA_VERSION: u32 = 1;

pub fn router() -> Router<Arc<CodeIntelState>> {
    Router::new()
        .route("/v1/help", get(help_route))
        .route("/v1/status", get(status_route))
        .route("/v1/index/status", get(index_status_route))
        .route("/v1/refresh", post(refresh_route))
        .route("/v1/outline", get(outline_route))
        .route("/v1/find", get(find_route))
        .route("/v1/def", get(nav::def_route))
        .route("/v1/refs", get(nav::refs_route))
        .route("/v1/search", get(search_route))
        .route("/v1/patch", post(patch_route))
        .route("/v1/pack", get(pack::pack_route))
}

async fn help_route() -> Json<HelpResponse> {
    Json(help_response(SCHEMA_VERSION))
}

pub async fn require_code_intel_auth(
    State(state): State<Arc<CodeIntelState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if bearer_from_headers(req.headers()) != Some(state.config.token.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    next.run(req).await
}

fn bearer_from_headers(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    if token.trim().is_empty() {
        None
    } else {
        Some(token)
    }
}

fn clean_str(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[derive(Debug, Deserialize)]
struct RootQuery {
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PathQuery {
    cwd: Option<String>,
    path: Option<String>,
    budget: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FindQuery {
    cwd: Option<String>,
    q: Option<String>,
    name: Option<String>,
    budget: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StructuralSearchQuery {
    cwd: Option<String>,
    lang: Option<String>,
    pattern: Option<String>,
    path: Option<String>,
    budget: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StructuralPatchRequest {
    cwd: Option<String>,
    lang: String,
    pattern: String,
    rewrite: String,
    path: Option<String>,
    budget: Option<String>,
}

async fn status_route(
    State(state): State<Arc<CodeIntelState>>,
    headers: HeaderMap,
    Query(query): Query<RootQuery>,
) -> Result<Json<StatusResponse>, CodeIntelError> {
    let target = resolve_target(
        &state.config.allowed_roots,
        &headers,
        query.cwd.as_deref(),
        None,
    )?;
    let summary = load_index_summary(&state.pool, &target.root).await?;
    let warnings = summary_warnings(&summary, false);
    Ok(Json(StatusResponse {
        schema_version: SCHEMA_VERSION,
        command: "status",
        root: RootView::from_spec(&target.root),
        freshness: freshness_for_summary(&summary),
        confidence: confidence_for_summary(&summary, false),
        warnings,
        index: IndexSummaryView::from_summary(summary),
        supported_languages: SourceLanguage::SUPPORTED
            .iter()
            .map(|language| language.as_str())
            .collect(),
        semantic: SemanticStatus::from_runtime(state.lsp.status()),
        examples: vec![
            "sulion-code refresh".to_string(),
            "sulion-code outline backend/src".to_string(),
            "sulion-code find <symbol-or-name>".to_string(),
        ],
    }))
}

async fn index_status_route(
    State(state): State<Arc<CodeIntelState>>,
    headers: HeaderMap,
    Query(query): Query<RootQuery>,
) -> Result<Json<IndexStatusResponse>, CodeIntelError> {
    let target = resolve_target(
        &state.config.allowed_roots,
        &headers,
        query.cwd.as_deref(),
        None,
    )?;
    let summary = load_index_summary(&state.pool, &target.root).await?;
    let warnings = summary_warnings(&summary, false);
    Ok(Json(IndexStatusResponse {
        schema_version: SCHEMA_VERSION,
        command: "index_status",
        root: RootView::from_spec(&target.root),
        freshness: freshness_for_summary(&summary),
        confidence: confidence_for_summary(&summary, false),
        warnings,
        index: IndexSummaryView::from_summary(summary),
    }))
}

async fn refresh_route(
    State(state): State<Arc<CodeIntelState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Json<RefreshResponse>, CodeIntelError> {
    let target = resolve_target(
        &state.config.allowed_roots,
        &headers,
        query.cwd.as_deref(),
        query.path.as_deref(),
    )?;
    let options = IndexOptions {
        trigger: IndexTrigger::Manual,
        ..IndexOptions::default()
    };
    let stats = refresh_target(state.clone(), target.clone(), options).await?;
    let summary = load_index_summary(&state.pool, &target.root).await?;
    Ok(Json(RefreshResponse {
        schema_version: SCHEMA_VERSION,
        command: "refresh",
        root: RootView::from_spec(&target.root),
        path: target.relative_path.clone(),
        freshness: freshness_for_summary(&summary),
        confidence: confidence_for_summary(&summary, false),
        warnings: summary_warnings(&summary, false),
        stats: RefreshStatsView::from_stats(stats),
    }))
}

async fn outline_route(
    State(state): State<Arc<CodeIntelState>>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Json<CommandResponse<SymbolResult>>, CodeIntelError> {
    let budget = Budget::parse(query.budget.as_deref())?;
    let target = resolve_target(
        &state.config.allowed_roots,
        &headers,
        query.cwd.as_deref(),
        query.path.as_deref(),
    )?;
    if target.kind == TargetKind::Missing {
        return Err(CodeIntelError::not_found(format!(
            "path not found: {}",
            target.target_path.display()
        )));
    }
    let summary = load_index_summary(&state.pool, &target.root).await?;
    let Some(root_id) = summary.root_id else {
        return Ok(Json(CommandResponse {
            schema_version: SCHEMA_VERSION,
            command: "outline",
            root: RootView::from_spec(&target.root),
            freshness: freshness_for_summary(&summary),
            confidence: confidence_for_summary(&summary, false),
            warnings: summary_warnings(&summary, false),
            truncated: false,
            results: Vec::new(),
        }));
    };
    let (results, truncated) = load_outline_symbols(&state.pool, root_id, &target, budget).await?;
    Ok(Json(CommandResponse {
        schema_version: SCHEMA_VERSION,
        command: "outline",
        root: RootView::from_spec(&target.root),
        freshness: freshness_for_summary(&summary),
        confidence: confidence_for_summary(&summary, truncated),
        warnings: summary_warnings(&summary, truncated),
        truncated,
        results,
    }))
}

async fn find_route(
    State(state): State<Arc<CodeIntelState>>,
    headers: HeaderMap,
    Query(query): Query<FindQuery>,
) -> Result<Json<CommandResponse<SymbolResult>>, CodeIntelError> {
    let budget = Budget::parse(query.budget.as_deref())?;
    let needle = clean_str(query.q.as_deref())
        .or_else(|| clean_str(query.name.as_deref()))
        .ok_or_else(|| CodeIntelError::bad_request("find requires q"))?;
    let target = resolve_target(
        &state.config.allowed_roots,
        &headers,
        query.cwd.as_deref(),
        None,
    )?;
    let summary = load_index_summary(&state.pool, &target.root).await?;
    let Some(root_id) = summary.root_id else {
        return Ok(Json(CommandResponse {
            schema_version: SCHEMA_VERSION,
            command: "find",
            root: RootView::from_spec(&target.root),
            freshness: freshness_for_summary(&summary),
            confidence: confidence_for_summary(&summary, false),
            warnings: summary_warnings(&summary, false),
            truncated: false,
            results: Vec::new(),
        }));
    };
    let (results, truncated) = load_find_symbols(&state.pool, root_id, needle, budget).await?;
    Ok(Json(CommandResponse {
        schema_version: SCHEMA_VERSION,
        command: "find",
        root: RootView::from_spec(&target.root),
        freshness: freshness_for_summary(&summary),
        confidence: confidence_for_summary(&summary, truncated),
        warnings: summary_warnings(&summary, truncated),
        truncated,
        results,
    }))
}

async fn search_route(
    State(state): State<Arc<CodeIntelState>>,
    headers: HeaderMap,
    Query(query): Query<StructuralSearchQuery>,
) -> Result<Json<CommandResponse<StructuralMatchResult>>, CodeIntelError> {
    let budget = Budget::parse(query.budget.as_deref())?;
    let language = parse_structural_language(query.lang.as_deref())?;
    let pattern = clean_str(query.pattern.as_deref())
        .ok_or_else(|| CodeIntelError::bad_request("search requires pattern"))?;
    let target = resolve_existing_target(
        &state.config.allowed_roots,
        &headers,
        query.cwd.as_deref(),
        query.path.as_deref(),
    )?;
    let files = structural::discover_structural_files(
        &target.root.path,
        &target.target_path,
        language,
        &SourceWalkOptions::default(),
    )?;
    let output =
        structural::search_files(&files, language, pattern, budget.result_limit() as usize);
    let summary = load_index_summary(&state.pool, &target.root).await?;
    let mut warnings = summary_warnings(&summary, output.truncated);
    warnings.extend(output.warnings);
    Ok(Json(CommandResponse {
        schema_version: SCHEMA_VERSION,
        command: "search",
        root: RootView::from_spec(&target.root),
        freshness: freshness_for_summary(&summary),
        confidence: confidence_for_summary(&summary, output.truncated || !warnings.is_empty()),
        warnings,
        truncated: output.truncated,
        results: output.results,
    }))
}

async fn patch_route(
    State(state): State<Arc<CodeIntelState>>,
    headers: HeaderMap,
    Json(request): Json<StructuralPatchRequest>,
) -> Result<Json<PatchResponse>, CodeIntelError> {
    let budget = Budget::parse(request.budget.as_deref())?;
    let language = StructuralLanguage::parse(&request.lang)
        .map_err(|err| CodeIntelError::bad_request(err.to_string()))?;
    if clean_str(Some(&request.pattern)).is_none() {
        return Err(CodeIntelError::bad_request("patch requires pattern"));
    }
    let target = resolve_existing_target(
        &state.config.allowed_roots,
        &headers,
        request.cwd.as_deref(),
        request.path.as_deref(),
    )?;
    let files = structural::discover_structural_files(
        &target.root.path,
        &target.target_path,
        language,
        &SourceWalkOptions::default(),
    )?;
    let output = structural::patch_files(
        &files,
        language,
        &request.pattern,
        &request.rewrite,
        budget.result_limit() as usize,
    );
    let summary = load_index_summary(&state.pool, &target.root).await?;
    let mut warnings = summary_warnings(&summary, output.truncated);
    warnings.extend(output.warnings);
    Ok(Json(PatchResponse {
        schema_version: SCHEMA_VERSION,
        command: "patch",
        root: RootView::from_spec(&target.root),
        freshness: freshness_for_summary(&summary),
        confidence: confidence_for_summary(&summary, output.truncated || !warnings.is_empty()),
        warnings,
        truncated: output.truncated,
        matches: output.matches,
        applied: false,
        diff: output.diff,
        files: output.files,
    }))
}

fn parse_structural_language(value: Option<&str>) -> Result<StructuralLanguage, CodeIntelError> {
    let value =
        clean_str(value).ok_or_else(|| CodeIntelError::bad_request("search requires lang"))?;
    StructuralLanguage::parse(value).map_err(|err| CodeIntelError::bad_request(err.to_string()))
}

fn resolve_existing_target(
    allowed_roots: &[PathBuf],
    headers: &HeaderMap,
    cwd: Option<&str>,
    path: Option<&str>,
) -> Result<ResolvedTarget, CodeIntelError> {
    let target = resolve_target(allowed_roots, headers, cwd, path)?;
    if target.kind == TargetKind::Missing {
        return Err(CodeIntelError::not_found(format!(
            "path not found: {}",
            target.target_path.display()
        )));
    }
    Ok(target)
}

async fn refresh_target(
    state: Arc<CodeIntelState>,
    target: ResolvedTarget,
    options: IndexOptions,
) -> anyhow::Result<RefreshStats> {
    let _guard = state.index_lock.lock().await;
    refresh_target_inner(&state.pool, &target, &options).await
}

async fn refresh_target_inner(
    pool: &Pool,
    target: &ResolvedTarget,
    options: &IndexOptions,
) -> anyhow::Result<RefreshStats> {
    if target.relative_path.is_none() {
        indexer::mark_root_dirty(pool, &target.root, options).await
    } else {
        indexer::mark_path_dirty(pool, &target.root, &target.target_path, options).await
    }
}

async fn load_index_summary(
    pool: &Pool,
    root: &CodeRootSpec,
) -> Result<IndexSummary, CodeIntelError> {
    let row = sqlx::query(
        // Aggregate code_files and code_symbols independently (LATERAL per table)
        // rather than joining both to the root, which would fan out to
        // files × symbols rows and force COUNT(DISTINCT) over that product —
        // O(files·symbols) and the cause of multi-second status calls.
        "SELECT cr.id, cr.last_scan_at, \
                f.latest_indexed_at, \
                f.file_count, \
                f.pending_file_count, \
                f.deleted_file_count, \
                s.symbol_count, \
                f.partial_file_count, \
                f.failed_file_count \
           FROM code_roots cr \
           LEFT JOIN LATERAL ( \
                SELECT MAX(indexed_at) FILTER (WHERE deleted_at IS NULL) AS latest_indexed_at, \
                       COUNT(*) FILTER (WHERE deleted_at IS NULL) AS file_count, \
                       COUNT(*) FILTER (WHERE deleted_at IS NULL AND parse_status = 'pending') AS pending_file_count, \
                       COUNT(*) FILTER (WHERE deleted_at IS NOT NULL OR parse_status = 'deleted') AS deleted_file_count, \
                       COUNT(*) FILTER (WHERE deleted_at IS NULL AND parse_status = 'partial') AS partial_file_count, \
                       COUNT(*) FILTER (WHERE deleted_at IS NULL AND parse_status = 'failed') AS failed_file_count \
                  FROM code_files WHERE root_id = cr.id \
           ) f ON TRUE \
           LEFT JOIN LATERAL ( \
                SELECT COUNT(*) AS symbol_count FROM code_symbols WHERE root_id = cr.id \
           ) s ON TRUE \
          WHERE cr.path = $1 AND cr.deleted_at IS NULL",
    )
    .bind(root.path.to_string_lossy().as_ref())
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(IndexSummary {
            root_id: None,
            last_scan_at: None,
            latest_indexed_at: None,
            file_count: 0,
            pending_file_count: 0,
            deleted_file_count: 0,
            symbol_count: 0,
            partial_file_count: 0,
            failed_file_count: 0,
            latest_job: None,
        });
    };
    let root_id: Uuid = row.get("id");
    Ok(IndexSummary {
        root_id: Some(root_id),
        last_scan_at: row.try_get("last_scan_at").ok().flatten(),
        latest_indexed_at: row.try_get("latest_indexed_at").ok().flatten(),
        file_count: row.try_get("file_count").unwrap_or(0),
        pending_file_count: row.try_get("pending_file_count").unwrap_or(0),
        deleted_file_count: row.try_get("deleted_file_count").unwrap_or(0),
        symbol_count: row.try_get("symbol_count").unwrap_or(0),
        partial_file_count: row.try_get("partial_file_count").unwrap_or(0),
        failed_file_count: row.try_get("failed_file_count").unwrap_or(0),
        latest_job: load_latest_job(pool, root_id).await?,
    })
}

async fn load_latest_job(
    pool: &Pool,
    root_id: Uuid,
) -> Result<Option<IndexJobView>, CodeIntelError> {
    let row = sqlx::query(
        "SELECT status, trigger, started_at, finished_at, files_seen, files_indexed, \
                files_failed, error \
           FROM code_index_jobs \
          WHERE root_id = $1 \
          ORDER BY created_at DESC \
          LIMIT 1",
    )
    .bind(root_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| IndexJobView {
        status: row.try_get("status").unwrap_or_default(),
        trigger: row.try_get("trigger").unwrap_or_default(),
        started_at: row.try_get("started_at").ok().flatten(),
        finished_at: row.try_get("finished_at").ok().flatten(),
        files_seen: row.try_get::<i32, _>("files_seen").unwrap_or(0) as i64,
        files_indexed: row.try_get::<i32, _>("files_indexed").unwrap_or(0) as i64,
        files_failed: row.try_get::<i32, _>("files_failed").unwrap_or(0) as i64,
        error: row.try_get("error").ok().flatten(),
    }))
}

async fn load_root_id(pool: &Pool, root: &CodeRootSpec) -> Result<Uuid, CodeIntelError> {
    sqlx::query_scalar("SELECT id FROM code_roots WHERE path = $1 AND deleted_at IS NULL")
        .bind(root.path.to_string_lossy().as_ref())
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| CodeIntelError::not_found("index root not found; run sulion-code refresh"))
}

async fn load_outline_symbols(
    pool: &Pool,
    root_id: Uuid,
    target: &ResolvedTarget,
    budget: Budget,
) -> Result<(Vec<SymbolResult>, bool), CodeIntelError> {
    let limit = budget.result_limit();
    let fetch_limit = limit + 1;
    let relative = target.relative_path.as_deref();
    let prefix_pattern =
        relative.map(|path| format!("{}/%", escape_like(path.trim_end_matches('/'))));
    let rows = sqlx::query(
        "SELECT s.id, s.parent_symbol_id, s.kind, s.name, s.qualified_name, s.signature, \
                s.visibility, s.exported, s.decl_start_line, s.decl_start_col, \
                s.decl_end_line, s.decl_end_col, s.body_start_line, s.body_start_col, \
                s.body_end_line, s.body_end_col, s.confidence, f.path, f.parse_status, \
                f.parse_error_count \
           FROM code_symbols s \
           JOIN code_files f ON f.id = s.file_id \
          WHERE s.root_id = $1 \
            AND f.deleted_at IS NULL \
            AND ( \
              $2::TEXT IS NULL \
              OR ($4::BOOLEAN AND f.path = $2) \
              OR ((NOT $4::BOOLEAN) AND (f.path = $2 OR f.path LIKE $3 ESCAPE '\\')) \
            ) \
          ORDER BY f.path, s.decl_start_line, s.decl_start_col, s.qualified_name \
          LIMIT $5",
    )
    .bind(root_id)
    .bind(relative)
    .bind(prefix_pattern.as_deref())
    .bind(target.kind == TargetKind::File)
    .bind(fetch_limit)
    .fetch_all(pool)
    .await?;
    Ok(rows_to_symbol_results(rows, limit))
}

async fn load_find_symbols(
    pool: &Pool,
    root_id: Uuid,
    needle: &str,
    budget: Budget,
) -> Result<(Vec<SymbolResult>, bool), CodeIntelError> {
    let limit = budget.result_limit();
    let fetch_limit = limit + 1;
    let prefix_pattern = format!("{}%", escape_like(needle));
    let contains_pattern = format!("%{}%", escape_like(needle));
    let rows = sqlx::query(
        "SELECT s.id, s.parent_symbol_id, s.kind, s.name, s.qualified_name, s.signature, \
                s.visibility, s.exported, s.decl_start_line, s.decl_start_col, \
                s.decl_end_line, s.decl_end_col, s.body_start_line, s.body_start_col, \
                s.body_end_line, s.body_end_col, s.confidence, f.path, f.parse_status, \
                f.parse_error_count \
           FROM code_symbols s \
           JOIN code_files f ON f.id = s.file_id \
          WHERE s.root_id = $1 \
            AND f.deleted_at IS NULL \
            AND (s.name ILIKE $4 ESCAPE '\\' OR s.qualified_name ILIKE $4 ESCAPE '\\') \
          ORDER BY \
            CASE \
              WHEN s.name = $2 OR s.qualified_name = $2 THEN 0 \
              WHEN lower(s.name) = lower($2) OR lower(s.qualified_name) = lower($2) THEN 1 \
              WHEN s.name ILIKE $3 ESCAPE '\\' THEN 2 \
              WHEN s.qualified_name ILIKE $3 ESCAPE '\\' THEN 3 \
              ELSE 4 \
            END, \
            length(s.qualified_name), s.qualified_name, f.path \
          LIMIT $5",
    )
    .bind(root_id)
    .bind(needle)
    .bind(prefix_pattern)
    .bind(contains_pattern)
    .bind(fetch_limit)
    .fetch_all(pool)
    .await?;
    Ok(rows_to_symbol_results(rows, limit))
}

#[derive(Debug)]
struct CodeIntelError {
    pub(super) status: StatusCode,
    message: String,
}

impl CodeIntelError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for CodeIntelError {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!(%err, "code-intel internal error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal error".to_string(),
        }
    }
}

impl From<sqlx::Error> for CodeIntelError {
    fn from(err: sqlx::Error) -> Self {
        tracing::error!(%err, "code-intel database error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "database error".to_string(),
        }
    }
}

impl From<std::io::Error> for CodeIntelError {
    fn from(err: std::io::Error) -> Self {
        tracing::error!(%err, "code-intel io error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "io error".to_string(),
        }
    }
}

impl IntoResponse for CodeIntelError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::{Method, Request};
    use sqlx::postgres::PgPoolOptions;
    use std::path::PathBuf;
    use tower::ServiceExt;

    fn test_config(allowed_root: PathBuf) -> super::super::CodeIntelConfig {
        super::super::CodeIntelConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            db_url: String::new(),
            token: "test-token".to_string(),
            allowed_roots: vec![allowed_root],
        }
    }

    #[tokio::test]
    async fn protected_routes_require_bearer_auth() {
        let temp = tempfile::tempdir().unwrap();
        let repos = temp.path().join("repos");
        let repo = repos.join("sulion");
        std::fs::create_dir_all(&repo).unwrap();
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/sulion")
            .unwrap();
        let state = super::super::CodeIntelState::from_pool_for_tests(pool, test_config(repos));
        let app = super::super::app(state);
        let requests = [
            (Method::GET, "/v1/help".to_string(), Body::empty()),
            (
                Method::GET,
                format!("/v1/status?cwd={}", repo.display()),
                Body::empty(),
            ),
            (
                Method::GET,
                format!("/v1/index/status?cwd={}", repo.display()),
                Body::empty(),
            ),
            (
                Method::GET,
                format!(
                    "/v1/search?cwd={}&lang=rust&pattern=foo%28%24A%29",
                    repo.display()
                ),
                Body::empty(),
            ),
            (
                Method::GET,
                format!("/v1/def?cwd={}&target=src/lib.rs:1", repo.display()),
                Body::empty(),
            ),
            (
                Method::GET,
                format!("/v1/refs?cwd={}&target=src/lib.rs:1:1", repo.display()),
                Body::empty(),
            ),
            (
                Method::GET,
                format!("/v1/pack?cwd={}&target=src/lib.rs:1-1", repo.display()),
                Body::empty(),
            ),
            (
                Method::POST,
                "/v1/patch".to_string(),
                Body::from(
                    serde_json::json!({
                        "cwd": repo.to_string_lossy(),
                        "lang": "rust",
                        "pattern": "foo($A)",
                        "rewrite": "bar($A)"
                    })
                    .to_string(),
                ),
            ),
        ];

        for (method, uri, body) in requests {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .body(body)
                .unwrap();

            let response = app.clone().oneshot(request).await.unwrap();

            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn help_route_returns_command_contract() {
        let temp = tempfile::tempdir().unwrap();
        let repos = temp.path().join("repos");
        std::fs::create_dir_all(&repos).unwrap();
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/sulion")
            .unwrap();
        let state = super::super::CodeIntelState::from_pool_for_tests(pool, test_config(repos));
        let app = super::super::app(state);
        let request = Request::builder()
            .method(Method::GET)
            .uri("/v1/help")
            .header("authorization", "Bearer test-token")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["schema_version"], 1);
        assert_eq!(body["command"], "help");
        assert!(body["usage"].as_str().unwrap().contains("sulion-code"));
        assert!(body["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["name"] == "patch"));
    }
}
