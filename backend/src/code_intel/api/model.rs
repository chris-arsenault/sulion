use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use super::{clean_str, CodeIntelError};
use crate::code_intel::indexer::{CodeRootKind, CodeRootSpec, RefreshStats};
use crate::code_intel::lsp::{LspLocation, SemanticRuntimeStatus};
use crate::code_intel::structural::StructuralPatchFileSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Budget {
    Small,
    Normal,
    Large,
}

impl Budget {
    pub(super) fn parse(value: Option<&str>) -> Result<Self, CodeIntelError> {
        match clean_str(value).unwrap_or("normal") {
            "small" => Ok(Self::Small),
            "normal" => Ok(Self::Normal),
            "large" => Ok(Self::Large),
            other => Err(CodeIntelError::bad_request(format!(
                "unsupported budget {other}; use small, normal, or large"
            ))),
        }
    }

    pub(super) fn result_limit(self) -> i64 {
        match self {
            Self::Small => 50,
            Self::Normal => 200,
            Self::Large => 1000,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Normal => "normal",
            Self::Large => "large",
        }
    }

    pub(super) fn pack_excerpt_lines(self) -> i32 {
        match self {
            Self::Small => 20,
            Self::Normal => 80,
            Self::Large => 180,
        }
    }

    pub(super) fn pack_reference_limit(self) -> i64 {
        match self {
            Self::Small => 3,
            Self::Normal => 8,
            Self::Large => 20,
        }
    }

    pub(super) fn pack_import_limit(self) -> usize {
        match self {
            Self::Small => 8,
            Self::Normal => 16,
            Self::Large => 32,
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct StatusResponse {
    pub(super) schema_version: u32,
    pub(super) command: &'static str,
    pub(super) root: RootView,
    pub(super) freshness: Freshness,
    pub(super) confidence: Confidence,
    pub(super) warnings: Vec<String>,
    pub(super) index: IndexSummaryView,
    pub(super) supported_languages: Vec<&'static str>,
    pub(super) semantic: SemanticStatus,
    pub(super) examples: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct IndexStatusResponse {
    pub(super) schema_version: u32,
    pub(super) command: &'static str,
    pub(super) root: RootView,
    pub(super) freshness: Freshness,
    pub(super) confidence: Confidence,
    pub(super) warnings: Vec<String>,
    pub(super) index: IndexSummaryView,
}

#[derive(Debug, Serialize)]
pub(super) struct RefreshResponse {
    pub(super) schema_version: u32,
    pub(super) command: &'static str,
    pub(super) root: RootView,
    pub(super) path: Option<String>,
    pub(super) freshness: Freshness,
    pub(super) confidence: Confidence,
    pub(super) warnings: Vec<String>,
    pub(super) stats: RefreshStatsView,
}

#[derive(Debug, Serialize)]
pub(super) struct CommandResponse<T> {
    pub(super) schema_version: u32,
    pub(super) command: &'static str,
    pub(super) root: RootView,
    pub(super) freshness: Freshness,
    pub(super) confidence: Confidence,
    pub(super) warnings: Vec<String>,
    pub(super) truncated: bool,
    pub(super) results: Vec<T>,
}

#[derive(Debug, Serialize)]
pub(super) struct PatchResponse {
    pub(super) schema_version: u32,
    pub(super) command: &'static str,
    pub(super) root: RootView,
    pub(super) freshness: Freshness,
    pub(super) confidence: Confidence,
    pub(super) warnings: Vec<String>,
    pub(super) truncated: bool,
    pub(super) matches: usize,
    pub(super) applied: bool,
    pub(super) diff: String,
    pub(super) files: Vec<StructuralPatchFileSummary>,
}

#[derive(Debug, Serialize)]
pub(super) struct PackResponse {
    pub(super) schema_version: u32,
    pub(super) command: &'static str,
    pub(super) root: RootView,
    pub(super) freshness: Freshness,
    pub(super) confidence: Confidence,
    pub(super) warnings: Vec<String>,
    pub(super) truncated: bool,
    pub(super) budget: &'static str,
    pub(super) bundle: PackBundle,
}

#[derive(Debug, Serialize)]
pub(super) struct RootView {
    kind: &'static str,
    name: String,
    path: String,
}

impl RootView {
    pub(super) fn from_spec(root: &CodeRootSpec) -> Self {
        Self {
            kind: match root.kind {
                CodeRootKind::Repo => "repo",
                CodeRootKind::Workspace => "workspace",
            },
            name: root.name.clone(),
            path: root.path.to_string_lossy().into_owned(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct SemanticStatus {
    pub(super) available: bool,
    pub(super) languages: Vec<SemanticLanguageStatus>,
    pub(super) reason: Option<String>,
    pub(super) timeout_ms: u64,
    pub(super) fallback: &'static str,
}

impl SemanticStatus {
    pub(super) fn from_runtime(status: SemanticRuntimeStatus) -> Self {
        Self {
            available: status.available,
            languages: status
                .languages
                .into_iter()
                .map(|language| SemanticLanguageStatus {
                    language: language.language,
                    command: language.command,
                    available: language.available,
                    health: language.health,
                    startup: language.startup,
                    last_error: language.last_error,
                })
                .collect(),
            reason: status.reason,
            timeout_ms: status.timeout_ms,
            fallback: status.fallback,
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct SemanticLanguageStatus {
    language: &'static str,
    command: String,
    available: bool,
    health: &'static str,
    startup: &'static str,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Freshness {
    Fresh,
    Stale,
    Partial,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Confidence {
    Semantic,
    Syntactic,
    Mixed,
    Stale,
    Partial,
}

#[derive(Debug, Serialize)]
pub(super) struct IndexSummaryView {
    last_scan_at: Option<DateTime<Utc>>,
    latest_indexed_at: Option<DateTime<Utc>>,
    file_count: i64,
    pending_file_count: i64,
    deleted_file_count: i64,
    symbol_count: i64,
    partial_file_count: i64,
    failed_file_count: i64,
    latest_job: Option<IndexJobView>,
}

impl IndexSummaryView {
    pub(super) fn from_summary(summary: IndexSummary) -> Self {
        Self {
            last_scan_at: summary.last_scan_at,
            latest_indexed_at: summary.latest_indexed_at,
            file_count: summary.file_count,
            pending_file_count: summary.pending_file_count,
            deleted_file_count: summary.deleted_file_count,
            symbol_count: summary.symbol_count,
            partial_file_count: summary.partial_file_count,
            failed_file_count: summary.failed_file_count,
            latest_job: summary.latest_job,
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct IndexJobView {
    pub(super) status: String,
    pub(super) trigger: String,
    pub(super) started_at: Option<DateTime<Utc>>,
    pub(super) finished_at: Option<DateTime<Utc>>,
    pub(super) files_seen: i64,
    pub(super) files_indexed: i64,
    pub(super) files_failed: i64,
    pub(super) error: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct RefreshStatsView {
    files_seen: usize,
    files_marked_pending: usize,
    files_deleted: usize,
}

impl RefreshStatsView {
    pub(super) fn from_stats(stats: RefreshStats) -> Self {
        Self {
            files_seen: stats.files_seen,
            files_marked_pending: stats.files_marked_pending,
            files_deleted: stats.files_deleted,
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct SymbolResult {
    id: String,
    kind: String,
    name: String,
    qualified_name: String,
    signature: Option<String>,
    range: RangeView,
    body_range: Option<RangeView>,
    parent_id: Option<String>,
    visibility: Option<String>,
    exported: Option<bool>,
    confidence: Confidence,
    freshness: Freshness,
}

impl SymbolResult {
    pub(super) fn semantic_location(kind: &str, name: &str, location: &LspLocation) -> Self {
        Self {
            id: format!(
                "semantic:{}:{}:{}",
                location.path, location.start_line, location.start_col
            ),
            kind: kind.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            signature: None,
            range: RangeView::from_lsp(location),
            body_range: None,
            parent_id: None,
            visibility: None,
            exported: None,
            confidence: Confidence::Semantic,
            freshness: Freshness::Fresh,
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct ReferenceResult {
    symbol_id: Option<String>,
    referenced_name: String,
    reference_kind: String,
    range: RangeView,
    confidence: Confidence,
    freshness: Freshness,
}

impl ReferenceResult {
    pub(super) fn semantic_location(referenced_name: &str, location: &LspLocation) -> Self {
        Self {
            symbol_id: None,
            referenced_name: referenced_name.to_string(),
            reference_kind: "semantic".to_string(),
            range: RangeView::from_lsp(location),
            confidence: Confidence::Semantic,
            freshness: Freshness::Fresh,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct PackBundle {
    pub(super) target: PackTargetView,
    pub(super) primary: Option<PackSymbolView>,
    pub(super) containers: Vec<PackSymbolView>,
    pub(super) imports: Vec<PackImportView>,
    pub(super) excerpt: PackExcerptView,
    pub(super) references: Vec<PackReferenceView>,
    pub(super) nearby_tests: Vec<PackReferenceView>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct PackTargetView {
    pub(super) kind: &'static str,
    pub(super) query: String,
    pub(super) range: RangeView,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct PackSymbolView {
    pub(super) id: String,
    pub(super) kind: String,
    pub(super) name: String,
    pub(super) qualified_name: String,
    pub(super) signature: Option<String>,
    pub(super) range: RangeView,
    pub(super) body_range: Option<RangeView>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct PackImportView {
    pub(super) path: String,
    pub(super) line: i32,
    pub(super) text: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct PackExcerptView {
    pub(super) range: RangeView,
    pub(super) text: String,
    pub(super) truncated_before: bool,
    pub(super) truncated_after: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct PackReferenceView {
    pub(super) path: String,
    pub(super) start_line: i32,
    pub(super) start_col: i32,
    pub(super) end_line: i32,
    pub(super) end_col: i32,
    pub(super) label: String,
    pub(super) kind: String,
    pub(super) confidence: Confidence,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct RangeView {
    pub(super) path: String,
    pub(super) start_line: i32,
    pub(super) start_col: i32,
    pub(super) end_line: i32,
    pub(super) end_col: i32,
}

impl RangeView {
    pub(super) fn new(
        path: impl Into<String>,
        start_line: i32,
        start_col: i32,
        end_line: i32,
        end_col: i32,
    ) -> Self {
        Self {
            path: path.into(),
            start_line,
            start_col,
            end_line,
            end_col,
        }
    }

    fn from_lsp(location: &LspLocation) -> Self {
        Self {
            path: location.path.clone(),
            start_line: location.start_line,
            start_col: location.start_col,
            end_line: location.end_line,
            end_col: location.end_col,
        }
    }
}

pub(super) fn rows_to_reference_results(
    rows: Vec<sqlx::postgres::PgRow>,
    limit: i64,
) -> (Vec<ReferenceResult>, bool) {
    let truncated = rows.len() > limit as usize;
    let results = rows
        .into_iter()
        .take(limit as usize)
        .map(reference_result_from_row)
        .collect();
    (results, truncated)
}

fn reference_result_from_row(row: sqlx::postgres::PgRow) -> ReferenceResult {
    let path: String = row.try_get("path").unwrap_or_default();
    let parse_status: String = row.try_get("parse_status").unwrap_or_default();
    let parse_error_count: i32 = row.try_get("parse_error_count").unwrap_or(0);
    let confidence = if parse_status == "pending" {
        Confidence::Stale
    } else if parse_status == "partial" || parse_error_count > 0 {
        Confidence::Partial
    } else {
        confidence_from_db(row.try_get("confidence").unwrap_or("syntactic"))
    };
    ReferenceResult {
        symbol_id: row.try_get("symbol_id").ok().flatten(),
        referenced_name: row.try_get("referenced_name").unwrap_or_default(),
        reference_kind: row.try_get("reference_kind").unwrap_or_default(),
        range: RangeView {
            path,
            start_line: row.try_get("start_line").unwrap_or(1),
            start_col: row.try_get("start_col").unwrap_or(1),
            end_line: row.try_get("end_line").unwrap_or(1),
            end_col: row.try_get("end_col").unwrap_or(1),
        },
        confidence,
        freshness: freshness_from_confidence(confidence),
    }
}

#[derive(Debug)]
pub(super) struct IndexSummary {
    pub(super) root_id: Option<Uuid>,
    pub(super) last_scan_at: Option<DateTime<Utc>>,
    pub(super) latest_indexed_at: Option<DateTime<Utc>>,
    pub(super) file_count: i64,
    pub(super) pending_file_count: i64,
    pub(super) deleted_file_count: i64,
    pub(super) symbol_count: i64,
    pub(super) partial_file_count: i64,
    pub(super) failed_file_count: i64,
    pub(super) latest_job: Option<IndexJobView>,
}

pub(super) fn rows_to_symbol_results(
    rows: Vec<sqlx::postgres::PgRow>,
    limit: i64,
) -> (Vec<SymbolResult>, bool) {
    let truncated = rows.len() > limit as usize;
    let results = rows
        .into_iter()
        .take(limit as usize)
        .map(symbol_result_from_row)
        .collect();
    (results, truncated)
}

fn symbol_result_from_row(row: sqlx::postgres::PgRow) -> SymbolResult {
    let path: String = row.try_get("path").unwrap_or_default();
    let parse_status: String = row.try_get("parse_status").unwrap_or_default();
    let parse_error_count: i32 = row.try_get("parse_error_count").unwrap_or(0);
    let confidence = if parse_status == "pending" {
        Confidence::Stale
    } else if parse_status == "partial" || parse_error_count > 0 {
        Confidence::Partial
    } else {
        confidence_from_db(row.try_get("confidence").unwrap_or("syntactic"))
    };
    SymbolResult {
        id: row.try_get("id").unwrap_or_default(),
        kind: row.try_get("kind").unwrap_or_default(),
        name: row.try_get("name").unwrap_or_default(),
        qualified_name: row.try_get("qualified_name").unwrap_or_default(),
        signature: row.try_get("signature").ok().flatten(),
        range: RangeView {
            path: path.clone(),
            start_line: row.try_get("decl_start_line").unwrap_or(1),
            start_col: row.try_get("decl_start_col").unwrap_or(1),
            end_line: row.try_get("decl_end_line").unwrap_or(1),
            end_col: row.try_get("decl_end_col").unwrap_or(1),
        },
        body_range: optional_range(&row, &path),
        parent_id: row.try_get("parent_symbol_id").ok().flatten(),
        visibility: row.try_get("visibility").ok().flatten(),
        exported: row.try_get("exported").ok().flatten(),
        confidence,
        freshness: freshness_from_confidence(confidence),
    }
}

fn optional_range(row: &sqlx::postgres::PgRow, path: &str) -> Option<RangeView> {
    let start_line: Option<i32> = row.try_get("body_start_line").ok().flatten();
    Some(RangeView {
        path: path.to_string(),
        start_line: start_line?,
        start_col: row.try_get("body_start_col").ok().flatten()?,
        end_line: row.try_get("body_end_line").ok().flatten()?,
        end_col: row.try_get("body_end_col").ok().flatten()?,
    })
}

pub(super) fn confidence_from_db(value: &str) -> Confidence {
    match value {
        "semantic" => Confidence::Semantic,
        "mixed" => Confidence::Mixed,
        "stale" => Confidence::Stale,
        "partial" => Confidence::Partial,
        _ => Confidence::Syntactic,
    }
}

fn freshness_from_confidence(confidence: Confidence) -> Freshness {
    match confidence {
        Confidence::Stale => Freshness::Stale,
        Confidence::Partial => Freshness::Partial,
        Confidence::Semantic | Confidence::Syntactic | Confidence::Mixed => Freshness::Fresh,
    }
}

pub(super) fn freshness_for_summary(summary: &IndexSummary) -> Freshness {
    if summary.root_id.is_none() || summary.last_scan_at.is_none() || summary.pending_file_count > 0
    {
        Freshness::Stale
    } else if summary.partial_file_count > 0 || summary.failed_file_count > 0 {
        Freshness::Partial
    } else {
        Freshness::Fresh
    }
}

pub(super) fn confidence_for_summary(
    summary: &IndexSummary,
    truncated_or_failed: bool,
) -> Confidence {
    if summary.root_id.is_none() || summary.last_scan_at.is_none() || summary.pending_file_count > 0
    {
        Confidence::Stale
    } else if truncated_or_failed || summary.partial_file_count > 0 || summary.failed_file_count > 0
    {
        Confidence::Partial
    } else {
        Confidence::Syntactic
    }
}

pub(super) fn summary_warnings(summary: &IndexSummary, truncated: bool) -> Vec<String> {
    let mut warnings = Vec::new();
    if summary.root_id.is_none() || summary.last_scan_at.is_none() {
        warnings.push("index is missing or stale; run sulion-code refresh".to_string());
    }
    if summary.pending_file_count > 0 {
        warnings.push(format!(
            "{} files are pending indexing; results may be stale",
            summary.pending_file_count
        ));
    }
    if summary.partial_file_count > 0 {
        warnings.push(format!(
            "{} files have parse errors; results may be partial",
            summary.partial_file_count
        ));
    }
    if summary.failed_file_count > 0 {
        warnings.push(format!(
            "{} files failed to index; results may be partial",
            summary.failed_file_count
        ));
    }
    if truncated {
        warnings.push("result budget truncated the response".to_string());
    }
    warnings
}

pub(super) fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_limits_are_stable() {
        assert_eq!(Budget::parse(None).unwrap(), Budget::Normal);
        assert_eq!(Budget::parse(Some("small")).unwrap().result_limit(), 50);
        assert_eq!(Budget::parse(Some("normal")).unwrap().result_limit(), 200);
        assert_eq!(Budget::parse(Some("large")).unwrap().result_limit(), 1000);
        assert!(Budget::parse(Some("huge")).is_err());
    }

    #[test]
    fn like_patterns_escape_postgres_wildcards() {
        assert_eq!(escape_like("foo_bar%baz\\qux"), "foo\\_bar\\%baz\\\\qux");
    }
}
