use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use ring::digest;
use sqlx::Row;
use uuid::Uuid;

use super::parser::{
    discover_source_files, source_file_candidate, ParseStatus, SourceFileCandidate, SourceParser,
    SourceWalkOptions,
};
use super::symbols::{extract_references, extract_symbols, ExtractedReference, ExtractedSymbol};
use super::CodeIntelState;
use crate::db::Pool;

const INDEXER_VERSION: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeRootKind {
    Repo,
    Workspace,
}

impl CodeRootKind {
    fn as_db_str(self) -> &'static str {
        match self {
            Self::Repo => "repo",
            Self::Workspace => "workspace",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeRootSpec {
    pub kind: CodeRootKind,
    pub name: String,
    pub path: PathBuf,
    pub repo_name: Option<String>,
    pub workspace_id: Option<Uuid>,
    pub git_head: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexTrigger {
    Startup,
    Manual,
    Query,
    Background,
}

impl IndexTrigger {
    fn as_db_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Manual => "manual",
            Self::Query => "query",
            Self::Background => "background",
        }
    }
}

#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub walk: SourceWalkOptions,
    pub trigger: IndexTrigger,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            walk: SourceWalkOptions::default(),
            trigger: IndexTrigger::Manual,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IndexStats {
    pub files_seen: usize,
    pub files_indexed: usize,
    pub files_skipped_unchanged: usize,
    pub files_deleted: usize,
    pub files_failed: usize,
    pub symbols_indexed: usize,
}

pub async fn run_background_indexer(state: Arc<CodeIntelState>, interval: Duration) {
    loop {
        let options = IndexOptions {
            trigger: IndexTrigger::Background,
            ..IndexOptions::default()
        };
        if let Err(err) =
            index_allowed_roots(&state.pool, &state.config.allowed_roots, &options).await
        {
            tracing::warn!(%err, "code-intel background index failed");
        }
        tokio::time::sleep(interval).await;
    }
}

pub async fn index_allowed_roots(
    pool: &Pool,
    allowed_roots: &[PathBuf],
    options: &IndexOptions,
) -> anyhow::Result<IndexStats> {
    let roots = discover_allowed_root_specs(allowed_roots)?;
    let mut total = IndexStats::default();
    for root in roots {
        total += index_root(pool, &root, options).await?;
    }
    Ok(total)
}

pub async fn index_root(
    pool: &Pool,
    root: &CodeRootSpec,
    options: &IndexOptions,
) -> anyhow::Result<IndexStats> {
    let root_id = upsert_root(pool, root).await?;
    let job_id = start_job(pool, root_id, options.trigger, None).await?;
    let result = index_root_inner(pool, root_id, root, options, IndexScope::Root).await;
    finish_job(pool, job_id, &result).await?;
    result
}

pub async fn index_path(
    pool: &Pool,
    root: &CodeRootSpec,
    absolute_path: &Path,
    options: &IndexOptions,
) -> anyhow::Result<IndexStats> {
    if !absolute_path.starts_with(&root.path) {
        anyhow::bail!(
            "{} is outside code root {}",
            absolute_path.display(),
            root.path.display()
        );
    }
    let root_id = upsert_root(pool, root).await?;
    let job_path = relative_path(&root.path, absolute_path)
        .ok()
        .filter(|path| !path.is_empty());
    let job_id = start_job(pool, root_id, options.trigger, job_path.as_deref()).await?;
    let result = index_root_inner(
        pool,
        root_id,
        root,
        options,
        IndexScope::Path(absolute_path.to_path_buf()),
    )
    .await;
    finish_job(pool, job_id, &result).await?;
    result
}

enum IndexScope {
    Root,
    Path(PathBuf),
}

async fn index_root_inner(
    pool: &Pool,
    root_id: Uuid,
    root: &CodeRootSpec,
    options: &IndexOptions,
    scope: IndexScope,
) -> anyhow::Result<IndexStats> {
    let (candidates, deletion_scope) = discover_scope_candidates(root, &options.walk, scope)?;
    let mut parser = SourceParser::default();
    let mut stats = IndexStats {
        files_seen: candidates.len(),
        ..IndexStats::default()
    };
    let mut seen_paths = HashSet::new();
    for candidate in candidates {
        let relative_path = relative_path(&root.path, &candidate.path)?;
        seen_paths.insert(relative_path.clone());
        match index_file(
            pool,
            root_id,
            root,
            &candidate.path,
            &relative_path,
            &mut parser,
        )
        .await
        {
            Ok(FileIndexOutcome::Indexed { symbols }) => {
                stats.files_indexed += 1;
                stats.symbols_indexed += symbols;
            }
            Ok(FileIndexOutcome::SkippedUnchanged) => {
                stats.files_skipped_unchanged += 1;
            }
            Err(err) => {
                tracing::warn!(path = %candidate.path.display(), %err, "code-intel file index failed");
                stats.files_failed += 1;
                mark_file_failed(pool, root_id, &relative_path, err.to_string()).await?;
            }
        }
    }
    stats.files_deleted = mark_deleted_files(pool, root_id, &seen_paths, deletion_scope).await?;
    sqlx::query("UPDATE code_roots SET last_scan_at = NOW(), updated_at = NOW() WHERE id = $1")
        .bind(root_id)
        .execute(pool)
        .await?;
    Ok(stats)
}

enum DeletionScope {
    Root,
    Prefix(String),
    Exact(String),
    MissingPath(String),
}

fn discover_scope_candidates(
    root: &CodeRootSpec,
    walk: &SourceWalkOptions,
    scope: IndexScope,
) -> anyhow::Result<(Vec<SourceFileCandidate>, DeletionScope)> {
    match scope {
        IndexScope::Root => Ok((
            discover_source_files(&root.path, walk)?,
            DeletionScope::Root,
        )),
        IndexScope::Path(path) if path == root.path => Ok((
            discover_source_files(&root.path, walk)?,
            DeletionScope::Root,
        )),
        IndexScope::Path(path) if path.is_dir() => {
            let relative = relative_path(&root.path, &path)?;
            let deletion_scope = if relative.is_empty() {
                DeletionScope::Root
            } else {
                DeletionScope::Prefix(format!("{}/", relative.trim_end_matches('/')))
            };
            Ok((discover_source_files(&path, walk)?, deletion_scope))
        }
        IndexScope::Path(path) if path.is_file() => {
            let relative = relative_path(&root.path, &path)?;
            let candidates = source_file_candidate(&path, walk)?.into_iter().collect();
            Ok((candidates, DeletionScope::Exact(relative)))
        }
        IndexScope::Path(path) => {
            let relative = relative_path(&root.path, &path)?;
            Ok((Vec::new(), DeletionScope::MissingPath(relative)))
        }
    }
}

enum FileIndexOutcome {
    Indexed { symbols: usize },
    SkippedUnchanged,
}

async fn index_file(
    pool: &Pool,
    root_id: Uuid,
    root: &CodeRootSpec,
    absolute_path: &Path,
    relative_path: &str,
    parser: &mut SourceParser,
) -> anyhow::Result<FileIndexOutcome> {
    let source = fs::read_to_string(absolute_path)
        .with_context(|| format!("read {}", absolute_path.display()))?;
    let content_hash = hash_bytes(source.as_bytes());
    if existing_hash(pool, root_id, relative_path)
        .await?
        .as_deref()
        == Some(content_hash.as_str())
    {
        return Ok(FileIndexOutcome::SkippedUnchanged);
    }
    let language = super::parser::language_required(absolute_path)?;
    let metadata =
        fs::metadata(absolute_path).with_context(|| format!("stat {}", absolute_path.display()))?;
    let parsed = parser.parse(language, &source)?;
    let symbols = extract_symbols(&parsed, &source, &root.path, relative_path);
    let references = extract_references(&parsed, &source, &symbols);
    let file_id = upsert_file(
        pool,
        root_id,
        relative_path,
        language.as_str(),
        &content_hash,
        metadata.len(),
        metadata.modified().ok(),
        parsed.line_index.line_count(),
        parsed.status,
        parsed.error_count,
    )
    .await?;
    replace_symbols(pool, root_id, file_id, &symbols, &references).await?;
    Ok(FileIndexOutcome::Indexed {
        symbols: symbols.len(),
    })
}

pub fn discover_allowed_root_specs(allowed_roots: &[PathBuf]) -> anyhow::Result<Vec<CodeRootSpec>> {
    let mut roots = Vec::new();
    for allowed_root in allowed_roots {
        if !allowed_root.is_dir() {
            continue;
        }
        for entry in fs::read_dir(allowed_root)
            .with_context(|| format!("read dir {}", allowed_root.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_dir() || is_generated_dir(&path) {
                continue;
            }
            let Some(name) = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let kind = if allowed_root.file_name().and_then(|name| name.to_str()) == Some("repos") {
                CodeRootKind::Repo
            } else {
                CodeRootKind::Workspace
            };
            roots.push(CodeRootSpec {
                kind,
                name: name.clone(),
                path,
                repo_name: Some(name),
                workspace_id: None,
                git_head: None,
            });
        }
    }
    roots.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(roots)
}

pub fn resolve_current_root(allowed_roots: &[PathBuf]) -> anyhow::Result<CodeRootSpec> {
    let cwd = std::env::current_dir().context("read current directory")?;
    if let Some(workspace_path) = env_path("SULION_WORKSPACE_PATH") {
        if cwd.starts_with(&workspace_path) {
            return Ok(CodeRootSpec {
                kind: CodeRootKind::Workspace,
                name: env_optional("SULION_REPO_NAME").unwrap_or_else(|| basename(&workspace_path)),
                path: workspace_path,
                repo_name: env_optional("SULION_REPO_NAME"),
                workspace_id: env_optional("SULION_WORKSPACE_ID")
                    .and_then(|value| value.parse::<Uuid>().ok()),
                git_head: env_optional("SULION_BASE_SHA"),
            });
        }
    }
    if let Some(repo_path) = env_path("SULION_CANONICAL_REPO") {
        if cwd.starts_with(&repo_path) {
            return Ok(CodeRootSpec {
                kind: CodeRootKind::Repo,
                name: env_optional("SULION_REPO_NAME").unwrap_or_else(|| basename(&repo_path)),
                path: repo_path,
                repo_name: env_optional("SULION_REPO_NAME"),
                workspace_id: None,
                git_head: None,
            });
        }
    }
    for allowed_root in allowed_roots {
        if let Ok(rest) = cwd.strip_prefix(allowed_root) {
            let Some(first) = rest.components().next() else {
                continue;
            };
            let root_path = allowed_root.join(first.as_os_str());
            let name = basename(&root_path);
            let kind = if allowed_root.file_name().and_then(|name| name.to_str()) == Some("repos") {
                CodeRootKind::Repo
            } else {
                CodeRootKind::Workspace
            };
            return Ok(CodeRootSpec {
                kind,
                name: name.clone(),
                path: root_path,
                repo_name: Some(name),
                workspace_id: None,
                git_head: None,
            });
        }
    }
    Err(anyhow!(
        "current directory {} is outside allowed code-intel roots",
        cwd.display()
    ))
}

fn is_generated_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git" | "target" | "node_modules" | "dist")
    )
}

async fn upsert_root(pool: &Pool, root: &CodeRootSpec) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        "INSERT INTO code_roots \
         (root_kind, name, path, repo_name, workspace_id, git_head, deleted_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, NULL, NOW()) \
         ON CONFLICT (path) WHERE deleted_at IS NULL DO UPDATE SET \
           root_kind = EXCLUDED.root_kind, \
           name = EXCLUDED.name, \
           repo_name = EXCLUDED.repo_name, \
           workspace_id = EXCLUDED.workspace_id, \
           git_head = EXCLUDED.git_head, \
           updated_at = NOW() \
         RETURNING id",
    )
    .bind(root.kind.as_db_str())
    .bind(&root.name)
    .bind(root.path.to_string_lossy().as_ref())
    .bind(&root.repo_name)
    .bind(root.workspace_id)
    .bind(&root.git_head)
    .fetch_one(pool)
    .await?;
    Ok(row.get("id"))
}

async fn start_job(
    pool: &Pool,
    root_id: Uuid,
    trigger: IndexTrigger,
    path: Option<&str>,
) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        "INSERT INTO code_index_jobs (root_id, status, trigger, path, started_at) \
         VALUES ($1, 'running', $2, $3, NOW()) RETURNING id",
    )
    .bind(root_id)
    .bind(trigger.as_db_str())
    .bind(path)
    .fetch_one(pool)
    .await?;
    Ok(row.get("id"))
}

async fn finish_job(
    pool: &Pool,
    job_id: Uuid,
    result: &anyhow::Result<IndexStats>,
) -> anyhow::Result<()> {
    match result {
        Ok(stats) => {
            sqlx::query(
                "UPDATE code_index_jobs SET \
                   status = 'complete', finished_at = NOW(), updated_at = NOW(), \
                   files_seen = $2, files_indexed = $3, files_failed = $4 \
                 WHERE id = $1",
            )
            .bind(job_id)
            .bind(stats.files_seen as i32)
            .bind(stats.files_indexed as i32)
            .bind(stats.files_failed as i32)
            .execute(pool)
            .await?;
        }
        Err(err) => {
            sqlx::query(
                "UPDATE code_index_jobs SET \
                   status = 'failed', finished_at = NOW(), updated_at = NOW(), error = $2 \
                 WHERE id = $1",
            )
            .bind(job_id)
            .bind(err.to_string())
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

async fn existing_hash(
    pool: &Pool,
    root_id: Uuid,
    relative_path: &str,
) -> anyhow::Result<Option<String>> {
    let row = sqlx::query(
        "SELECT content_hash FROM code_files \
         WHERE root_id = $1 AND path = $2 AND deleted_at IS NULL \
           AND metadata->>'indexer_version' = $3",
    )
    .bind(root_id)
    .bind(relative_path)
    .bind(INDEXER_VERSION.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|row| row.get("content_hash")))
}

#[allow(clippy::too_many_arguments)]
async fn upsert_file(
    pool: &Pool,
    root_id: Uuid,
    relative_path: &str,
    language: &str,
    content_hash: &str,
    size_bytes: u64,
    modified: Option<SystemTime>,
    line_count: usize,
    parse_status: ParseStatus,
    parse_error_count: usize,
) -> anyhow::Result<Uuid> {
    let mtime = modified.map(DateTime::<Utc>::from);
    let row = sqlx::query(
        "INSERT INTO code_files \
         (root_id, path, language, content_hash, size_bytes, mtime, line_count, \
          parse_status, parse_error_count, metadata, indexed_at, deleted_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, \
                 jsonb_build_object('indexer_version', $10::int), NOW(), NULL, NOW()) \
         ON CONFLICT (root_id, path) DO UPDATE SET \
           language = EXCLUDED.language, \
           content_hash = EXCLUDED.content_hash, \
           size_bytes = EXCLUDED.size_bytes, \
           mtime = EXCLUDED.mtime, \
           line_count = EXCLUDED.line_count, \
           parse_status = EXCLUDED.parse_status, \
           parse_error_count = EXCLUDED.parse_error_count, \
           metadata = EXCLUDED.metadata, \
           indexed_at = NOW(), \
           deleted_at = NULL, \
           updated_at = NOW() \
         RETURNING id",
    )
    .bind(root_id)
    .bind(relative_path)
    .bind(language)
    .bind(content_hash)
    .bind(size_bytes as i64)
    .bind(mtime)
    .bind(line_count as i32)
    .bind(parse_status.as_db_str())
    .bind(parse_error_count as i32)
    .bind(INDEXER_VERSION)
    .fetch_one(pool)
    .await?;
    Ok(row.get("id"))
}

async fn replace_symbols(
    pool: &Pool,
    root_id: Uuid,
    file_id: Uuid,
    symbols: &[ExtractedSymbol],
    references: &[ExtractedReference],
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM code_imports WHERE file_id = $1")
        .bind(file_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM code_references WHERE file_id = $1")
        .bind(file_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM code_symbols WHERE file_id = $1")
        .bind(file_id)
        .execute(pool)
        .await?;
    for symbol in symbols {
        insert_symbol(pool, root_id, file_id, symbol).await?;
    }
    for reference in references {
        insert_reference(pool, root_id, file_id, reference).await?;
    }
    Ok(())
}

async fn insert_symbol(
    pool: &Pool,
    root_id: Uuid,
    file_id: Uuid,
    symbol: &ExtractedSymbol,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO code_symbols \
         (id, root_id, file_id, parent_symbol_id, kind, name, qualified_name, signature, \
          visibility, exported, disambiguator, decl_start_line, decl_start_col, decl_end_line, \
          decl_end_col, body_start_line, body_start_col, body_end_line, body_end_col, \
          doc_start_line, doc_start_col, doc_end_line, doc_end_col, confidence, updated_at) \
         VALUES \
         ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, \
          $16, $17, $18, $19, $20, $21, $22, $23, 'syntactic', NOW())",
    )
    .bind(&symbol.id)
    .bind(root_id)
    .bind(file_id)
    .bind(&symbol.parent_id)
    .bind(&symbol.kind)
    .bind(&symbol.name)
    .bind(&symbol.qualified_name)
    .bind(&symbol.signature)
    .bind(&symbol.visibility)
    .bind(symbol.exported)
    .bind(symbol.disambiguator)
    .bind(symbol.decl_range.start.line as i32)
    .bind(symbol.decl_range.start.column as i32)
    .bind(symbol.decl_range.end.line as i32)
    .bind(symbol.decl_range.end.column as i32)
    .bind(symbol.body_range.map(|range| range.start.line as i32))
    .bind(symbol.body_range.map(|range| range.start.column as i32))
    .bind(symbol.body_range.map(|range| range.end.line as i32))
    .bind(symbol.body_range.map(|range| range.end.column as i32))
    .bind(symbol.doc_range.map(|range| range.start.line as i32))
    .bind(symbol.doc_range.map(|range| range.start.column as i32))
    .bind(symbol.doc_range.map(|range| range.end.line as i32))
    .bind(symbol.doc_range.map(|range| range.end.column as i32))
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_reference(
    pool: &Pool,
    root_id: Uuid,
    file_id: Uuid,
    reference: &ExtractedReference,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO code_references \
         (root_id, file_id, symbol_id, referenced_name, reference_kind, start_line, \
          start_col, end_line, end_col, confidence) \
         VALUES ($1, $2, NULL, $3, $4, $5, $6, $7, $8, 'syntactic')",
    )
    .bind(root_id)
    .bind(file_id)
    .bind(&reference.referenced_name)
    .bind(&reference.reference_kind)
    .bind(reference.range.start.line as i32)
    .bind(reference.range.start.column as i32)
    .bind(reference.range.end.line as i32)
    .bind(reference.range.end.column as i32)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_file_failed(
    pool: &Pool,
    root_id: Uuid,
    relative_path: &str,
    error: String,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO code_files \
         (root_id, path, parse_status, parse_error_count, metadata, indexed_at, updated_at) \
         VALUES ($1, $2, 'failed', 1, \
                 jsonb_build_object('error', $3::text, 'indexer_version', $4::int), NOW(), NOW()) \
         ON CONFLICT (root_id, path) DO UPDATE SET \
           parse_status = 'failed', \
           parse_error_count = 1, \
           metadata = jsonb_build_object('error', $3::text, 'indexer_version', $4::int), \
           indexed_at = NOW(), \
           deleted_at = NULL, \
           updated_at = NOW()",
    )
    .bind(root_id)
    .bind(relative_path)
    .bind(error)
    .bind(INDEXER_VERSION)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_deleted_files(
    pool: &Pool,
    root_id: Uuid,
    seen_paths: &HashSet<String>,
    scope: DeletionScope,
) -> anyhow::Result<usize> {
    let rows =
        sqlx::query("SELECT id, path FROM code_files WHERE root_id = $1 AND deleted_at IS NULL")
            .bind(root_id)
            .fetch_all(pool)
            .await?;
    let mut deleted = 0;
    for row in rows {
        let file_id: Uuid = row.get("id");
        let path: String = row.get("path");
        let in_scope = match &scope {
            DeletionScope::Root => true,
            DeletionScope::Prefix(prefix) => path.starts_with(prefix),
            DeletionScope::Exact(exact) => path == *exact,
            DeletionScope::MissingPath(missing) => {
                path == *missing || path.starts_with(&format!("{}/", missing.trim_end_matches('/')))
            }
        };
        if !in_scope {
            continue;
        }
        if seen_paths.contains(&path) {
            continue;
        }
        sqlx::query("DELETE FROM code_imports WHERE file_id = $1")
            .bind(file_id)
            .execute(pool)
            .await?;
        sqlx::query("DELETE FROM code_references WHERE file_id = $1")
            .bind(file_id)
            .execute(pool)
            .await?;
        sqlx::query("DELETE FROM code_symbols WHERE file_id = $1")
            .bind(file_id)
            .execute(pool)
            .await?;
        sqlx::query(
            "UPDATE code_files SET parse_status = 'deleted', deleted_at = NOW(), updated_at = NOW() \
             WHERE id = $1",
        )
        .bind(file_id)
        .execute(pool)
        .await?;
        deleted += 1;
    }
    Ok(deleted)
}

fn relative_path(root: &Path, path: &Path) -> anyhow::Result<String> {
    let relative = path
        .strip_prefix(root)
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn hash_bytes(bytes: &[u8]) -> String {
    let hash = digest::digest(&digest::SHA256, bytes);
    hash.as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn basename(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("root")
        .to_string()
}

fn env_path(key: &str) -> Option<PathBuf> {
    env_optional(key).map(PathBuf::from)
}

fn env_optional(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

impl std::ops::AddAssign for IndexStats {
    fn add_assign(&mut self, rhs: Self) {
        self.files_seen += rhs.files_seen;
        self.files_indexed += rhs.files_indexed;
        self.files_skipped_unchanged += rhs.files_skipped_unchanged;
        self.files_deleted += rhs.files_deleted;
        self.files_failed += rhs.files_failed;
        self.symbols_indexed += rhs.symbols_indexed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_allowed_root_children_as_index_roots() {
        let temp = tempfile::tempdir().unwrap();
        let repos = temp.path().join("repos");
        fs::create_dir(&repos).unwrap();
        fs::create_dir(repos.join("sulion")).unwrap();
        fs::create_dir(repos.join("target")).unwrap();

        let roots = discover_allowed_root_specs(&[repos]).unwrap();

        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].kind, CodeRootKind::Repo);
        assert_eq!(roots[0].name, "sulion");
        assert_eq!(roots[0].repo_name.as_deref(), Some("sulion"));
    }

    #[test]
    fn content_hash_is_stable_and_sensitive_to_content() {
        assert_eq!(hash_bytes(b"abc"), hash_bytes(b"abc"));
        assert_ne!(hash_bytes(b"abc"), hash_bytes(b"abcd"));
    }
}
