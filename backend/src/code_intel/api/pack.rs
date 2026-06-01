use std::path::Path;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

use super::model::{
    confidence_for_summary, freshness_for_summary, summary_warnings, Budget, Confidence,
    PackBundle, PackExcerptView, PackImportView, PackReferenceView, PackResponse, PackSymbolView,
    PackTargetView, RangeView, RootView,
};
use super::root::{resolve_target, TargetKind};
use super::{
    clean_str, load_index_summary, load_root_id, refresh_target, CodeIntelError, SCHEMA_VERSION,
};
use crate::code_intel::indexer::{self, IndexOptions, IndexTrigger};
use crate::code_intel::CodeIntelState;
use crate::db::Pool;

#[derive(Debug, Deserialize)]
pub(super) struct PackQuery {
    cwd: Option<String>,
    target: Option<String>,
    budget: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PackTarget {
    SymbolId(String),
    Range {
        path: String,
        start_line: i32,
        end_line: i32,
    },
}

#[derive(Debug, Clone)]
struct IndexedSymbol {
    id: String,
    parent_id: Option<String>,
    kind: String,
    name: String,
    qualified_name: String,
    signature: Option<String>,
    decl_range: RangeView,
    body_range: Option<RangeView>,
}

#[derive(Debug, Clone)]
struct PackBuildInput {
    query: String,
    target_kind: &'static str,
    target_range: RangeView,
    excerpt_range: RangeView,
    primary: Option<IndexedSymbol>,
}

pub(super) async fn pack_route(
    State(state): State<Arc<CodeIntelState>>,
    headers: HeaderMap,
    Query(query): Query<PackQuery>,
) -> Result<Json<PackResponse>, CodeIntelError> {
    let budget = Budget::parse(query.budget.as_deref())?;
    let target_arg = clean_str(query.target.as_deref())
        .ok_or_else(|| CodeIntelError::bad_request("pack requires target"))?;
    let target = parse_pack_target(target_arg).map_err(CodeIntelError::bad_request)?;
    let (root, bundle, truncated) = match target {
        PackTarget::SymbolId(symbol_id) => {
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
            let symbol = load_symbol_by_id(&state.pool, root_id, &symbol_id)
                .await?
                .ok_or_else(|| {
                    CodeIntelError::not_found(format!("symbol not found for {symbol_id}"))
                })?;
            let excerpt_range = symbol
                .body_range
                .clone()
                .unwrap_or_else(|| symbol.decl_range.clone());
            let input = PackBuildInput {
                query: target_arg.to_string(),
                target_kind: "symbol",
                target_range: symbol.decl_range.clone(),
                excerpt_range,
                primary: Some(symbol),
            };
            let (bundle, truncated) =
                build_pack_bundle(&state.pool, root_id, &target.root.path, input, budget).await?;
            (target.root, bundle, truncated)
        }
        PackTarget::Range {
            path,
            start_line,
            end_line,
        } => {
            let target = super::resolve_existing_target(
                &state.config.allowed_roots,
                &headers,
                query.cwd.as_deref(),
                Some(&path),
            )?;
            if target.kind != TargetKind::File {
                return Err(CodeIntelError::bad_request(format!(
                    "pack range target must be a file: {path}"
                )));
            }
            let options = IndexOptions {
                trigger: IndexTrigger::Query,
                ..IndexOptions::default()
            };
            refresh_target(&state.pool, &target, &options).await?;
            let root_id = load_root_id(&state.pool, &target.root).await?;
            let relative_path = target.relative_path.as_deref().ok_or_else(|| {
                CodeIntelError::bad_request(format!("pack range target must be a file: {path}"))
            })?;
            let end_col = line_end_col(&target.target_path, end_line).await?;
            let target_range = RangeView::new(relative_path, start_line, 1, end_line, end_col);
            let primary =
                load_symbol_containing_range(&state.pool, root_id, relative_path, start_line)
                    .await?;
            let input = PackBuildInput {
                query: target_arg.to_string(),
                target_kind: "range",
                target_range: target_range.clone(),
                excerpt_range: target_range,
                primary,
            };
            let (bundle, truncated) =
                build_pack_bundle(&state.pool, root_id, &target.root.path, input, budget).await?;
            (target.root, bundle, truncated)
        }
    };
    let summary = load_index_summary(&state.pool, &root).await?;
    Ok(Json(PackResponse {
        schema_version: SCHEMA_VERSION,
        command: "pack",
        root: RootView::from_spec(&root),
        freshness: freshness_for_summary(&summary),
        confidence: confidence_for_summary(&summary, truncated),
        warnings: summary_warnings(&summary, truncated),
        truncated,
        budget: budget.as_str(),
        bundle,
    }))
}

fn parse_pack_target(value: &str) -> Result<PackTarget, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("pack target must not be empty".to_string());
    }
    if value.starts_with("sym_") && !value.contains(':') {
        return Ok(PackTarget::SymbolId(value.to_string()));
    }
    let Some((path, range)) = value.rsplit_once(':') else {
        return Err("pack target must be a symbol id or path:line-line".to_string());
    };
    let Some((start, end)) = range.split_once('-') else {
        return Err("pack range target must use path:line-line".to_string());
    };
    let start_line = parse_positive_line(start, "start line")?;
    let end_line = parse_positive_line(end, "end line")?;
    if end_line < start_line {
        return Err("pack range end line must be greater than or equal to start line".to_string());
    }
    let path = path.trim();
    if path.is_empty() {
        return Err("pack range path must not be empty".to_string());
    }
    Ok(PackTarget::Range {
        path: path.to_string(),
        start_line,
        end_line,
    })
}

fn parse_positive_line(value: &str, label: &str) -> Result<i32, String> {
    let line = value
        .trim()
        .parse::<i32>()
        .map_err(|_| format!("pack range {label} must be a positive integer"))?;
    if line < 1 {
        Err(format!("pack range {label} must be a positive integer"))
    } else {
        Ok(line)
    }
}

async fn build_pack_bundle(
    pool: &Pool,
    root_id: Uuid,
    root_path: &Path,
    input: PackBuildInput,
    budget: Budget,
) -> Result<(PackBundle, bool), CodeIntelError> {
    let primary_name = input.primary.as_ref().map(|symbol| symbol.name.as_str());
    let containers = match &input.primary {
        Some(symbol) => load_symbol_containers(pool, root_id, symbol.parent_id.as_deref()).await?,
        None => Vec::new(),
    };
    let imports = important_imports(root_path, &input.excerpt_range.path, budget).await?;
    let (excerpt, excerpt_truncated) =
        selected_excerpt(root_path, &input.excerpt_range, budget).await?;
    let (references, refs_truncated) = match primary_name {
        Some(name) => load_pack_references(pool, root_id, name, budget, false).await?,
        None => (Vec::new(), false),
    };
    let (nearby_tests, tests_truncated) = match primary_name {
        Some(name) => load_pack_references(pool, root_id, name, budget, true).await?,
        None => (Vec::new(), false),
    };
    let bundle = PackBundle {
        target: PackTargetView {
            kind: input.target_kind,
            query: input.query,
            range: input.target_range,
        },
        primary: input.primary.map(PackSymbolView::from),
        containers: containers.into_iter().map(PackSymbolView::from).collect(),
        imports,
        excerpt,
        references,
        nearby_tests,
    };
    Ok((
        bundle,
        excerpt_truncated || refs_truncated || tests_truncated,
    ))
}

async fn load_symbol_by_id(
    pool: &Pool,
    root_id: Uuid,
    symbol_id: &str,
) -> Result<Option<IndexedSymbol>, CodeIntelError> {
    let sql = symbol_select_sql(
        "s.root_id = $1 AND s.id = $2 AND f.deleted_at IS NULL",
        "LIMIT 1",
    );
    let row = sqlx::query(&sql)
        .bind(root_id)
        .bind(symbol_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(indexed_symbol_from_row))
}

async fn load_symbol_containing_range(
    pool: &Pool,
    root_id: Uuid,
    path: &str,
    line: i32,
) -> Result<Option<IndexedSymbol>, CodeIntelError> {
    let sql = symbol_select_sql(
        "s.root_id = $1 \
         AND f.path = $2 \
         AND f.deleted_at IS NULL \
         AND ( \
           ($3 BETWEEN s.decl_start_line AND s.decl_end_line) \
           OR (s.body_start_line IS NOT NULL AND $3 BETWEEN s.body_start_line AND s.body_end_line) \
         )",
        "ORDER BY \
           COALESCE(s.body_end_line, s.decl_end_line) - s.decl_start_line ASC, \
           s.decl_start_line DESC, s.decl_start_col DESC \
         LIMIT 1",
    );
    let row = sqlx::query(&sql)
        .bind(root_id)
        .bind(path)
        .bind(line)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(indexed_symbol_from_row))
}

async fn load_symbol_containers(
    pool: &Pool,
    root_id: Uuid,
    parent_id: Option<&str>,
) -> Result<Vec<IndexedSymbol>, CodeIntelError> {
    let mut containers = Vec::new();
    let mut next_parent = parent_id.map(str::to_string);
    for _ in 0..16 {
        let Some(symbol_id) = next_parent else {
            break;
        };
        let Some(symbol) = load_symbol_by_id(pool, root_id, &symbol_id).await? else {
            break;
        };
        next_parent = symbol.parent_id.clone();
        containers.push(symbol);
    }
    containers.reverse();
    Ok(containers)
}

fn symbol_select_sql(where_clause: &str, suffix: &str) -> String {
    format!(
        "SELECT s.id, s.parent_symbol_id, s.kind, s.name, s.qualified_name, s.signature, \
                s.decl_start_line, s.decl_start_col, s.decl_end_line, s.decl_end_col, \
                s.body_start_line, s.body_start_col, s.body_end_line, s.body_end_col, \
                f.path \
           FROM code_symbols s \
           JOIN code_files f ON f.id = s.file_id \
          WHERE {where_clause} \
          {suffix}"
    )
}

fn indexed_symbol_from_row(row: sqlx::postgres::PgRow) -> IndexedSymbol {
    let path: String = row.get("path");
    IndexedSymbol {
        id: row.get("id"),
        parent_id: row.try_get("parent_symbol_id").ok().flatten(),
        kind: row.get("kind"),
        name: row.get("name"),
        qualified_name: row.get("qualified_name"),
        signature: row.try_get("signature").ok().flatten(),
        decl_range: RangeView::new(
            path.clone(),
            row.get("decl_start_line"),
            row.get("decl_start_col"),
            row.get("decl_end_line"),
            row.get("decl_end_col"),
        ),
        body_range: optional_range_from_row(&row, &path),
    }
}

fn optional_range_from_row(row: &sqlx::postgres::PgRow, path: &str) -> Option<RangeView> {
    Some(RangeView::new(
        path,
        row.try_get("body_start_line").ok().flatten()?,
        row.try_get("body_start_col").ok().flatten()?,
        row.try_get("body_end_line").ok().flatten()?,
        row.try_get("body_end_col").ok().flatten()?,
    ))
}

impl From<IndexedSymbol> for PackSymbolView {
    fn from(symbol: IndexedSymbol) -> Self {
        Self {
            id: symbol.id,
            kind: symbol.kind,
            name: symbol.name,
            qualified_name: symbol.qualified_name,
            signature: symbol.signature,
            range: symbol.decl_range,
            body_range: symbol.body_range,
        }
    }
}

async fn important_imports(
    root_path: &Path,
    relative_path: &str,
    budget: Budget,
) -> Result<Vec<PackImportView>, CodeIntelError> {
    let source = tokio::fs::read_to_string(root_path.join(relative_path)).await?;
    Ok(extract_important_imports(
        relative_path,
        &source,
        budget.pack_import_limit(),
    ))
}

fn extract_important_imports(path: &str, source: &str, limit: usize) -> Vec<PackImportView> {
    source
        .lines()
        .enumerate()
        .filter_map(|(idx, line)| {
            let trimmed = line.trim();
            is_import_line(trimmed).then(|| PackImportView {
                path: path.to_string(),
                line: idx as i32 + 1,
                text: trimmed.to_string(),
            })
        })
        .take(limit)
        .collect()
}

fn is_import_line(line: &str) -> bool {
    line.starts_with("use ")
        || line.starts_with("pub use ")
        || line.starts_with("extern crate ")
        || line.starts_with("import ")
        || (line.starts_with("export ") && line.contains(" from "))
        || line.starts_with("require(")
        || line.starts_with("const ") && line.contains(" require(")
}

async fn selected_excerpt(
    root_path: &Path,
    range: &RangeView,
    budget: Budget,
) -> Result<(PackExcerptView, bool), CodeIntelError> {
    let source = tokio::fs::read_to_string(root_path.join(&range.path)).await?;
    Ok(select_excerpt(&source, range, budget.pack_excerpt_lines()))
}

fn select_excerpt(source: &str, range: &RangeView, max_lines: i32) -> (PackExcerptView, bool) {
    let lines = source.lines().collect::<Vec<_>>();
    let line_count = lines.len().max(1) as i32;
    let requested_start = range.start_line.clamp(1, line_count);
    let requested_end = range.end_line.clamp(requested_start, line_count);
    let requested_len = requested_end - requested_start + 1;
    let clipped_end = if requested_len > max_lines {
        requested_start + max_lines - 1
    } else {
        requested_end
    };
    let text = lines[(requested_start - 1) as usize..clipped_end as usize].join("\n");
    let excerpt_range = RangeView::new(
        range.path.clone(),
        requested_start,
        if requested_start == range.start_line {
            range.start_col
        } else {
            1
        },
        clipped_end,
        if clipped_end == range.end_line {
            range.end_col
        } else {
            lines
                .get((clipped_end - 1) as usize)
                .map(|line| line.len() as i32 + 1)
                .unwrap_or(1)
        },
    );
    let truncated_after = clipped_end < requested_end;
    (
        PackExcerptView {
            range: excerpt_range,
            text,
            truncated_before: requested_start > range.start_line,
            truncated_after,
        },
        truncated_after,
    )
}

async fn line_end_col(path: &Path, line: i32) -> Result<i32, CodeIntelError> {
    let source = tokio::fs::read_to_string(path).await?;
    Ok(source
        .lines()
        .nth(line.saturating_sub(1) as usize)
        .map(|line| line.len() as i32 + 1)
        .unwrap_or(1))
}

async fn load_pack_references(
    pool: &Pool,
    root_id: Uuid,
    name: &str,
    budget: Budget,
    tests_only: bool,
) -> Result<(Vec<PackReferenceView>, bool), CodeIntelError> {
    let limit = budget.pack_reference_limit();
    let fetch_limit = limit + 1;
    let rows = sqlx::query(
        "SELECT r.referenced_name, r.reference_kind, r.start_line, r.start_col, \
                r.end_line, r.end_col, r.confidence, f.path, f.parse_status, f.parse_error_count \
           FROM code_references r \
           JOIN code_files f ON f.id = r.file_id \
          WHERE r.root_id = $1 \
            AND r.referenced_name = $2 \
            AND f.deleted_at IS NULL \
            AND ( \
              NOT $3::BOOLEAN \
              OR f.path ILIKE '%test%' \
              OR f.path ILIKE '%spec%' \
              OR f.path ILIKE '%__tests__%' \
            ) \
          ORDER BY f.path, r.start_line, r.start_col \
          LIMIT $4",
    )
    .bind(root_id)
    .bind(name)
    .bind(tests_only)
    .bind(fetch_limit)
    .fetch_all(pool)
    .await?;
    let truncated = rows.len() > limit as usize;
    let results = rows
        .into_iter()
        .take(limit as usize)
        .map(pack_reference_from_row)
        .collect();
    Ok((results, truncated))
}

fn pack_reference_from_row(row: sqlx::postgres::PgRow) -> PackReferenceView {
    let parse_status: String = row.try_get("parse_status").unwrap_or_default();
    let parse_error_count: i32 = row.try_get("parse_error_count").unwrap_or(0);
    let confidence = if parse_status == "partial" || parse_error_count > 0 {
        Confidence::Partial
    } else {
        super::model::confidence_from_db(row.try_get("confidence").unwrap_or("syntactic"))
    };
    PackReferenceView {
        path: row.get("path"),
        start_line: row.get("start_line"),
        start_col: row.get("start_col"),
        end_line: row.get("end_line"),
        end_col: row.get("end_col"),
        label: row.get("referenced_name"),
        kind: row.get("reference_kind"),
        confidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pack_symbol_and_range_targets() {
        assert_eq!(
            parse_pack_target("sym_abc123").unwrap(),
            PackTarget::SymbolId("sym_abc123".to_string())
        );
        assert_eq!(
            parse_pack_target("backend/src/lib.rs:10-20").unwrap(),
            PackTarget::Range {
                path: "backend/src/lib.rs".to_string(),
                start_line: 10,
                end_line: 20,
            }
        );
        assert!(parse_pack_target("backend/src/lib.rs:20-10").is_err());
        assert!(parse_pack_target("backend/src/lib.rs:20").is_err());
    }

    #[test]
    fn select_excerpt_clips_to_budget_without_dumping_whole_source() {
        let source = (1..=12)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let range = RangeView::new("src/lib.rs", 2, 1, 10, 8);

        let (excerpt, truncated) = select_excerpt(&source, &range, 4);

        assert!(truncated);
        assert_eq!(excerpt.range.start_line, 2);
        assert_eq!(excerpt.range.end_line, 5);
        assert_eq!(excerpt.text, "line 2\nline 3\nline 4\nline 5");
        assert!(excerpt.truncated_after);
    }

    #[test]
    fn extracts_important_import_lines_with_limit() {
        let source = "use crate::api;\nfn main() {}\nimport x from 'x';\nconst y = require('y');\n";

        let imports = extract_important_imports("src/main.ts", source, 2);

        assert_eq!(imports.len(), 2);
        assert_eq!(imports[0].line, 1);
        assert_eq!(imports[1].line, 3);
    }
}
