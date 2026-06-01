use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

use super::model::{
    confidence_for_summary, freshness_for_summary, rows_to_reference_results,
    rows_to_symbol_results, summary_warnings, Budget, CommandResponse, Confidence, ReferenceResult,
    RootView, SymbolResult,
};
use super::root::{resolve_target, ResolvedTarget, TargetKind};
use super::{
    clean_str, load_index_summary, load_root_id, refresh_target, CodeIntelError, SCHEMA_VERSION,
};
use crate::code_intel::indexer::{self, IndexOptions, IndexTrigger};
use crate::code_intel::lsp::SemanticResponse;
use crate::code_intel::navigation::{
    first_identifier_column_on_line, identifier_at_position, parse_navigation_target,
    FilePositionTarget, NavigationTarget,
};
use crate::code_intel::CodeIntelState;
use crate::db::Pool;

#[derive(Debug, Deserialize)]
pub(super) struct NavigationQuery {
    cwd: Option<String>,
    target: Option<String>,
    budget: Option<String>,
}

pub(super) async fn def_route(
    State(state): State<Arc<CodeIntelState>>,
    headers: HeaderMap,
    Query(query): Query<NavigationQuery>,
) -> Result<Json<CommandResponse<SymbolResult>>, CodeIntelError> {
    let budget = Budget::parse(query.budget.as_deref())?;
    let target_arg = clean_str(query.target.as_deref())
        .ok_or_else(|| CodeIntelError::bad_request("def requires target"))?;
    let target = parse_navigation_target(target_arg).map_err(CodeIntelError::bad_request)?;
    let mut semantic_warnings = Vec::new();
    let mut semantic_used = false;
    let (root, results, truncated) = match target {
        NavigationTarget::SymbolId(symbol_id) => {
            let target = resolve_target(
                &state.config.allowed_roots,
                &headers,
                query.cwd.as_deref(),
                None,
            )?;
            let options = IndexOptions {
                trigger: IndexTrigger::Query,
                ..IndexOptions::default()
            };
            indexer::index_root(&state.pool, &target.root, &options).await?;
            let root_id = load_root_id(&state.pool, &target.root).await?;
            let symbol_target = load_symbol_navigation_target(&state.pool, root_id, &symbol_id)
                .await?
                .ok_or_else(|| {
                    CodeIntelError::not_found(format!("symbol not found for {symbol_id}"))
                })?;
            if let Some((results, truncated)) = semantic_definition(
                &state,
                &headers,
                query.cwd.as_deref(),
                &symbol_target.position,
                &symbol_target.name,
                budget,
                &mut semantic_warnings,
            )
            .await?
            {
                semantic_used = true;
                (target.root, results, truncated)
            } else {
                let (results, truncated) =
                    load_symbol_definition(&state.pool, root_id, &symbol_id, budget).await?;
                if results.is_empty() {
                    return Err(CodeIntelError::not_found(format!(
                        "definition not found for {symbol_id}"
                    )));
                }
                (target.root, results, truncated)
            }
        }
        NavigationTarget::Position(position) => {
            let target = resolve_position_target(
                &state.config.allowed_roots,
                &headers,
                query.cwd.as_deref(),
                &position,
            )?;
            let options = IndexOptions {
                trigger: IndexTrigger::Query,
                ..IndexOptions::default()
            };
            refresh_target(&state.pool, &target, &options).await?;
            let root_id = load_root_id(&state.pool, &target.root).await?;
            let semantic_name =
                reference_name_for_position(&state.pool, root_id, &target, &position)
                    .await?
                    .unwrap_or_else(|| "definition".to_string());
            if let Some((results, truncated)) = semantic_definition(
                &state,
                &headers,
                query.cwd.as_deref(),
                &position,
                &semantic_name,
                budget,
                &mut semantic_warnings,
            )
            .await?
            {
                semantic_used = true;
                (target.root, results, truncated)
            } else {
                let (results, truncated) =
                    load_definition_for_position(&state.pool, root_id, &target, &position, budget)
                        .await?;
                if results.is_empty() {
                    return Err(CodeIntelError::not_found(format!(
                        "definition not found for {}:{}",
                        position.path, position.line
                    )));
                }
                (target.root, results, truncated)
            }
        }
    };
    let summary = load_index_summary(&state.pool, &root).await?;
    let mut warnings = summary_warnings(&summary, truncated);
    warnings.extend(semantic_warnings);
    Ok(Json(CommandResponse {
        schema_version: SCHEMA_VERSION,
        command: "def",
        root: RootView::from_spec(&root),
        freshness: freshness_for_summary(&summary),
        confidence: response_confidence(&summary, truncated, semantic_used),
        warnings,
        truncated,
        results,
    }))
}

pub(super) async fn refs_route(
    State(state): State<Arc<CodeIntelState>>,
    headers: HeaderMap,
    Query(query): Query<NavigationQuery>,
) -> Result<Json<CommandResponse<ReferenceResult>>, CodeIntelError> {
    let budget = Budget::parse(query.budget.as_deref())?;
    let target_arg = clean_str(query.target.as_deref())
        .ok_or_else(|| CodeIntelError::bad_request("refs requires target"))?;
    let target = parse_navigation_target(target_arg).map_err(CodeIntelError::bad_request)?;
    let mut semantic_warnings = Vec::new();
    let (root, referenced_name, semantic_results) = match target {
        NavigationTarget::SymbolId(symbol_id) => {
            let target = resolve_target(
                &state.config.allowed_roots,
                &headers,
                query.cwd.as_deref(),
                None,
            )?;
            let options = IndexOptions {
                trigger: IndexTrigger::Query,
                ..IndexOptions::default()
            };
            indexer::index_root(&state.pool, &target.root, &options).await?;
            let root_id = load_root_id(&state.pool, &target.root).await?;
            let symbol_target = load_symbol_navigation_target(&state.pool, root_id, &symbol_id)
                .await?
                .ok_or_else(|| {
                    CodeIntelError::not_found(format!("symbol not found for {symbol_id}"))
                })?;
            let semantic_results = semantic_references(
                &state,
                &headers,
                query.cwd.as_deref(),
                &symbol_target.position,
                &symbol_target.name,
                budget,
                &mut semantic_warnings,
            )
            .await?;
            (target.root, symbol_target.name, semantic_results)
        }
        NavigationTarget::Position(position) => {
            let target = resolve_position_target(
                &state.config.allowed_roots,
                &headers,
                query.cwd.as_deref(),
                &position,
            )?;
            let options = IndexOptions {
                trigger: IndexTrigger::Query,
                ..IndexOptions::default()
            };
            indexer::index_root(&state.pool, &target.root, &options).await?;
            let root_id = load_root_id(&state.pool, &target.root).await?;
            let name = reference_name_for_position(&state.pool, root_id, &target, &position)
                .await?
                .ok_or_else(|| {
                    CodeIntelError::not_found(format!(
                        "reference target not found for {}:{}",
                        position.path, position.line
                    ))
                })?;
            let semantic_results = semantic_references(
                &state,
                &headers,
                query.cwd.as_deref(),
                &position,
                &name,
                budget,
                &mut semantic_warnings,
            )
            .await?;
            (target.root, name, semantic_results)
        }
    };
    let semantic_used = semantic_results.is_some();
    let root_id = load_root_id(&state.pool, &root).await?;
    let (results, truncated) = if let Some(results) = semantic_results {
        results
    } else {
        load_references_by_name(&state.pool, root_id, &referenced_name, budget).await?
    };
    let summary = load_index_summary(&state.pool, &root).await?;
    let mut warnings = summary_warnings(&summary, truncated);
    warnings.extend(semantic_warnings);
    Ok(Json(CommandResponse {
        schema_version: SCHEMA_VERSION,
        command: "refs",
        root: RootView::from_spec(&root),
        freshness: freshness_for_summary(&summary),
        confidence: response_confidence(&summary, truncated, semantic_used),
        warnings,
        truncated,
        results,
    }))
}

fn resolve_position_target(
    allowed_roots: &[std::path::PathBuf],
    headers: &HeaderMap,
    cwd: Option<&str>,
    position: &FilePositionTarget,
) -> Result<ResolvedTarget, CodeIntelError> {
    let target = super::resolve_existing_target(allowed_roots, headers, cwd, Some(&position.path))?;
    if target.kind != TargetKind::File {
        return Err(CodeIntelError::bad_request(format!(
            "target must be a file: {}",
            position.path
        )));
    }
    Ok(target)
}

fn response_confidence(
    summary: &super::model::IndexSummary,
    truncated: bool,
    semantic_used: bool,
) -> Confidence {
    if semantic_used && !truncated {
        Confidence::Semantic
    } else if semantic_used {
        Confidence::Partial
    } else {
        confidence_for_summary(summary, truncated)
    }
}

async fn semantic_definition(
    state: &Arc<CodeIntelState>,
    headers: &HeaderMap,
    cwd: Option<&str>,
    position: &FilePositionTarget,
    name: &str,
    budget: Budget,
    warnings: &mut Vec<String>,
) -> Result<Option<(Vec<SymbolResult>, bool)>, CodeIntelError> {
    let target = resolve_position_target(&state.config.allowed_roots, headers, cwd, position)?;
    let Some(col) = lsp_column(&target, position).await? else {
        warnings.push("semantic fallback: target line has no identifier column".to_string());
        return Ok(None);
    };
    match state
        .lsp
        .definition(&target.root, &target.target_path, position.line, col)
        .await
    {
        SemanticResponse::Results(locations) => {
            let (results, truncated) = semantic_symbol_results(name, locations, budget);
            if results.is_empty() {
                warnings.push(
                    "semantic fallback: language server returned no locations under the root"
                        .to_string(),
                );
                Ok(None)
            } else {
                Ok(Some((results, truncated)))
            }
        }
        other => {
            if let Some(warning) = other.fallback_warning() {
                warnings.push(warning);
            }
            Ok(None)
        }
    }
}

async fn semantic_references(
    state: &Arc<CodeIntelState>,
    headers: &HeaderMap,
    cwd: Option<&str>,
    position: &FilePositionTarget,
    name: &str,
    budget: Budget,
    warnings: &mut Vec<String>,
) -> Result<Option<(Vec<ReferenceResult>, bool)>, CodeIntelError> {
    let target = resolve_position_target(&state.config.allowed_roots, headers, cwd, position)?;
    let Some(col) = lsp_column(&target, position).await? else {
        warnings.push("semantic fallback: target line has no identifier column".to_string());
        return Ok(None);
    };
    match state
        .lsp
        .references(&target.root, &target.target_path, position.line, col)
        .await
    {
        SemanticResponse::Results(locations) => {
            let (results, truncated) = semantic_reference_results(name, locations, budget);
            if results.is_empty() {
                warnings.push(
                    "semantic fallback: language server returned no locations under the root"
                        .to_string(),
                );
                Ok(None)
            } else {
                Ok(Some((results, truncated)))
            }
        }
        other => {
            if let Some(warning) = other.fallback_warning() {
                warnings.push(warning);
            }
            Ok(None)
        }
    }
}

async fn lsp_column(
    target: &ResolvedTarget,
    position: &FilePositionTarget,
) -> Result<Option<i32>, CodeIntelError> {
    if position.col.is_some() {
        return Ok(position.col);
    }
    let source = tokio::fs::read_to_string(&target.target_path).await?;
    Ok(first_identifier_column_on_line(&source, position.line))
}

fn semantic_symbol_results(
    name: &str,
    locations: Vec<crate::code_intel::lsp::LspLocation>,
    budget: Budget,
) -> (Vec<SymbolResult>, bool) {
    let limit = budget.result_limit() as usize;
    let truncated = locations.len() > limit;
    let results = locations
        .into_iter()
        .take(limit)
        .map(|location| SymbolResult::semantic_location("definition", name, &location))
        .collect();
    (results, truncated)
}

fn semantic_reference_results(
    name: &str,
    locations: Vec<crate::code_intel::lsp::LspLocation>,
    budget: Budget,
) -> (Vec<ReferenceResult>, bool) {
    let limit = budget.result_limit() as usize;
    let truncated = locations.len() > limit;
    let results = locations
        .into_iter()
        .take(limit)
        .map(|location| ReferenceResult::semantic_location(name, &location))
        .collect();
    (results, truncated)
}

struct SymbolNavigationTarget {
    name: String,
    position: FilePositionTarget,
}

async fn load_symbol_navigation_target(
    pool: &Pool,
    root_id: Uuid,
    symbol_id: &str,
) -> Result<Option<SymbolNavigationTarget>, CodeIntelError> {
    let row = sqlx::query(
        "SELECT s.name, s.decl_start_line, s.decl_start_col, f.path \
           FROM code_symbols s \
           JOIN code_files f ON f.id = s.file_id \
          WHERE s.root_id = $1 \
            AND s.id = $2 \
            AND f.deleted_at IS NULL",
    )
    .bind(root_id)
    .bind(symbol_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| SymbolNavigationTarget {
        name: row.get("name"),
        position: FilePositionTarget {
            path: row.get("path"),
            line: row.get("decl_start_line"),
            col: row.try_get("decl_start_col").ok(),
        },
    }))
}

async fn load_symbol_definition(
    pool: &Pool,
    root_id: Uuid,
    symbol_id: &str,
    budget: Budget,
) -> Result<(Vec<SymbolResult>, bool), CodeIntelError> {
    let limit = budget.result_limit();
    let fetch_limit = limit + 1;
    let rows = sqlx::query(
        "SELECT s.id, s.parent_symbol_id, s.kind, s.name, s.qualified_name, s.signature, \
                s.visibility, s.exported, s.decl_start_line, s.decl_start_col, \
                s.decl_end_line, s.decl_end_col, s.body_start_line, s.body_start_col, \
                s.body_end_line, s.body_end_col, s.confidence, f.path, f.parse_status, \
                f.parse_error_count \
           FROM code_symbols s \
           JOIN code_files f ON f.id = s.file_id \
          WHERE s.root_id = $1 \
            AND s.id = $2 \
            AND f.deleted_at IS NULL \
          LIMIT $3",
    )
    .bind(root_id)
    .bind(symbol_id)
    .bind(fetch_limit)
    .fetch_all(pool)
    .await?;
    Ok(rows_to_symbol_results(rows, limit))
}

async fn load_definition_for_position(
    pool: &Pool,
    root_id: Uuid,
    target: &ResolvedTarget,
    position: &FilePositionTarget,
    budget: Budget,
) -> Result<(Vec<SymbolResult>, bool), CodeIntelError> {
    if position.col.is_some() {
        let source = std::fs::read_to_string(&target.target_path)?;
        if let Some(name) = identifier_at_position(&source, position.line, position.col) {
            return load_symbols_by_name(
                pool,
                root_id,
                &name,
                target.relative_path.as_deref(),
                budget,
            )
            .await;
        }
    }
    load_symbols_containing_position(pool, root_id, target, position, budget).await
}

async fn load_symbols_by_name(
    pool: &Pool,
    root_id: Uuid,
    name: &str,
    preferred_path: Option<&str>,
    budget: Budget,
) -> Result<(Vec<SymbolResult>, bool), CodeIntelError> {
    let limit = budget.result_limit();
    let fetch_limit = limit + 1;
    let qualified_suffix = format!("%::{}", super::escape_like(name));
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
            AND (s.name = $2 OR s.qualified_name = $2 OR s.qualified_name LIKE $3 ESCAPE '\\') \
          ORDER BY \
            CASE WHEN $4::TEXT IS NOT NULL AND f.path = $4 THEN 0 ELSE 1 END, \
            CASE WHEN s.name = $2 THEN 0 WHEN s.qualified_name = $2 THEN 1 ELSE 2 END, \
            length(s.qualified_name), s.qualified_name, f.path \
          LIMIT $5",
    )
    .bind(root_id)
    .bind(name)
    .bind(qualified_suffix)
    .bind(preferred_path)
    .bind(fetch_limit)
    .fetch_all(pool)
    .await?;
    Ok(rows_to_symbol_results(rows, limit))
}

async fn load_symbols_containing_position(
    pool: &Pool,
    root_id: Uuid,
    target: &ResolvedTarget,
    position: &FilePositionTarget,
    budget: Budget,
) -> Result<(Vec<SymbolResult>, bool), CodeIntelError> {
    let limit = budget.result_limit();
    let fetch_limit = limit + 1;
    let relative_path = target.relative_path.as_deref().ok_or_else(|| {
        CodeIntelError::bad_request(format!("target must be a file: {}", position.path))
    })?;
    let rows = sqlx::query(
        "SELECT s.id, s.parent_symbol_id, s.kind, s.name, s.qualified_name, s.signature, \
                s.visibility, s.exported, s.decl_start_line, s.decl_start_col, \
                s.decl_end_line, s.decl_end_col, s.body_start_line, s.body_start_col, \
                s.body_end_line, s.body_end_col, s.confidence, f.path, f.parse_status, \
                f.parse_error_count \
           FROM code_symbols s \
           JOIN code_files f ON f.id = s.file_id \
          WHERE s.root_id = $1 \
            AND f.path = $2 \
            AND f.deleted_at IS NULL \
            AND ( \
              ($3 BETWEEN s.decl_start_line AND s.decl_end_line) \
              OR (s.body_start_line IS NOT NULL AND $3 BETWEEN s.body_start_line AND s.body_end_line) \
            ) \
          ORDER BY s.decl_start_line DESC, s.decl_end_line ASC, s.decl_start_col DESC \
          LIMIT $4",
    )
    .bind(root_id)
    .bind(relative_path)
    .bind(position.line)
    .bind(fetch_limit)
    .fetch_all(pool)
    .await?;
    Ok(rows_to_symbol_results(rows, limit))
}

async fn reference_name_for_position(
    pool: &Pool,
    root_id: Uuid,
    target: &ResolvedTarget,
    position: &FilePositionTarget,
) -> Result<Option<String>, CodeIntelError> {
    if position.col.is_some() {
        let source = std::fs::read_to_string(&target.target_path)?;
        if let Some(name) = identifier_at_position(&source, position.line, position.col) {
            return Ok(Some(name));
        }
    }
    load_symbol_name_at_position(pool, root_id, target, position).await
}

async fn load_symbol_name_at_position(
    pool: &Pool,
    root_id: Uuid,
    target: &ResolvedTarget,
    position: &FilePositionTarget,
) -> Result<Option<String>, CodeIntelError> {
    let relative_path = target.relative_path.as_deref().ok_or_else(|| {
        CodeIntelError::bad_request(format!("target must be a file: {}", position.path))
    })?;
    let row = sqlx::query(
        "SELECT s.name \
           FROM code_symbols s \
           JOIN code_files f ON f.id = s.file_id \
          WHERE s.root_id = $1 \
            AND f.path = $2 \
            AND f.deleted_at IS NULL \
            AND ( \
              ($3 BETWEEN s.decl_start_line AND s.decl_end_line) \
              OR (s.body_start_line IS NOT NULL AND $3 BETWEEN s.body_start_line AND s.body_end_line) \
            ) \
          ORDER BY s.decl_start_line DESC, s.decl_end_line ASC, s.decl_start_col DESC \
          LIMIT 1",
    )
    .bind(root_id)
    .bind(relative_path)
    .bind(position.line)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row.get("name")))
}

async fn load_references_by_name(
    pool: &Pool,
    root_id: Uuid,
    referenced_name: &str,
    budget: Budget,
) -> Result<(Vec<ReferenceResult>, bool), CodeIntelError> {
    let limit = budget.result_limit();
    let fetch_limit = limit + 1;
    let rows = sqlx::query(
        "SELECT r.symbol_id, r.referenced_name, r.reference_kind, r.start_line, \
                r.start_col, r.end_line, r.end_col, r.confidence, f.path, \
                f.parse_status, f.parse_error_count \
           FROM code_references r \
           JOIN code_files f ON f.id = r.file_id \
          WHERE r.root_id = $1 \
            AND r.referenced_name = $2 \
            AND f.deleted_at IS NULL \
          ORDER BY f.path, r.start_line, r.start_col \
          LIMIT $3",
    )
    .bind(root_id)
    .bind(referenced_name)
    .bind(fetch_limit)
    .fetch_all(pool)
    .await?;
    Ok(rows_to_reference_results(rows, limit))
}
