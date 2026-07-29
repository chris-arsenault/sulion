use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Context;
use chrono::{DateTime, Utc};
use ring::digest;
use sqlx::Row;
use uuid::Uuid;

use super::parser::{
    discover_source_files, is_ignored_dir_name, is_ignored_path_in_root,
    source_file_candidate_in_root, ParseStatus, SourceFileCandidate, SourceParser,
    SourceWalkOptions,
};
use super::symbols::{extract_references, extract_symbols, ExtractedReference, ExtractedSymbol};
use super::CodeIntelState;
use crate::db::Pool;

mod storage;

use storage::*;

const INDEXER_VERSION: i32 = 2;
const INDEX_BATCH_LIMIT: i64 = 128;
const BACKGROUND_BUSY_DELAY: Duration = Duration::from_secs(1);

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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RefreshStats {
    pub files_seen: usize,
    pub files_marked_pending: usize,
    pub files_deleted: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryMode {
    DetectChanges,
    ForcePending,
}

pub async fn run_background_indexer(state: Arc<CodeIntelState>, interval: Duration) {
    loop {
        let options = IndexOptions {
            trigger: IndexTrigger::Background,
            ..IndexOptions::default()
        };
        let delay = {
            let _guard = state.index_lock.lock().await;
            match index_pending_allowed_roots(&state.pool, &state.config.allowed_roots, &options)
                .await
            {
                Ok(stats) if stats_has_indexing_work(&stats) => BACKGROUND_BUSY_DELAY,
                Ok(_) => interval,
                Err(err) => {
                    tracing::warn!(%err, "code-intel background index failed");
                    interval
                }
            }
        };
        tokio::time::sleep(delay).await;
    }
}

pub async fn run_startup_and_background_indexer(state: Arc<CodeIntelState>, interval: Duration) {
    if let Err(err) = run_startup_indexer_once(state.clone()).await {
        tracing::warn!(%err, "code-intel startup index failed");
    }
    run_background_indexer(state, interval).await;
}

pub async fn run_startup_indexer_once(state: Arc<CodeIntelState>) -> anyhow::Result<IndexStats> {
    let options = IndexOptions {
        trigger: IndexTrigger::Startup,
        ..IndexOptions::default()
    };
    let _guard = state.index_lock.lock().await;
    let stats = index_allowed_roots(&state.pool, &state.config.allowed_roots, &options).await?;
    tracing::info!(
        files_seen = stats.files_seen,
        files_indexed = stats.files_indexed,
        files_skipped_unchanged = stats.files_skipped_unchanged,
        files_deleted = stats.files_deleted,
        files_failed = stats.files_failed,
        symbols_indexed = stats.symbols_indexed,
        "code-intel startup index pass complete"
    );
    Ok(stats)
}

fn stats_has_indexing_work(stats: &IndexStats) -> bool {
    stats.files_seen > 0
        || stats.files_indexed > 0
        || stats.files_skipped_unchanged > 0
        || stats.files_failed > 0
}

pub async fn cancel_orphaned_running_jobs(pool: &Pool) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE code_index_jobs \
            SET status = 'cancelled', \
                finished_at = NOW(), \
                updated_at = NOW(), \
                error = COALESCE(error, 'service restarted before index job finished') \
          WHERE status = 'running'",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn index_allowed_roots(
    pool: &Pool,
    allowed_roots: &[PathBuf],
    options: &IndexOptions,
) -> anyhow::Result<IndexStats> {
    let roots = discover_allowed_root_specs(allowed_roots)?;
    let mut total = IndexStats::default();
    for root in roots {
        mark_root_dirty_inner(pool, &root, options, DiscoveryMode::DetectChanges).await?;
        total += index_pending_root(pool, &root, options, INDEX_BATCH_LIMIT).await?;
    }
    Ok(total)
}

pub async fn index_pending_allowed_roots(
    pool: &Pool,
    allowed_roots: &[PathBuf],
    options: &IndexOptions,
) -> anyhow::Result<IndexStats> {
    let roots = discover_allowed_root_specs(allowed_roots)?;
    let mut total = IndexStats::default();
    for root in roots {
        total += index_pending_root(pool, &root, options, INDEX_BATCH_LIMIT).await?;
    }
    Ok(total)
}

pub async fn mark_root_dirty(
    pool: &Pool,
    root: &CodeRootSpec,
    options: &IndexOptions,
) -> anyhow::Result<RefreshStats> {
    let root_id = upsert_root(pool, root).await?;
    mark_dirty_inner(
        pool,
        root_id,
        root,
        options,
        IndexScope::Root,
        DiscoveryMode::ForcePending,
    )
    .await
}
pub async fn mark_path_dirty(
    pool: &Pool,
    root: &CodeRootSpec,
    absolute_path: &Path,
    options: &IndexOptions,
) -> anyhow::Result<RefreshStats> {
    if !absolute_path.starts_with(&root.path) {
        anyhow::bail!(
            "{} is outside code root {}",
            absolute_path.display(),
            root.path.display()
        );
    }
    let root_id = upsert_root(pool, root).await?;
    mark_dirty_inner(
        pool,
        root_id,
        root,
        options,
        IndexScope::Path(absolute_path.to_path_buf()),
        DiscoveryMode::ForcePending,
    )
    .await
}

enum IndexScope {
    Root,
    Path(PathBuf),
}

async fn mark_root_dirty_inner(
    pool: &Pool,
    root: &CodeRootSpec,
    options: &IndexOptions,
    mode: DiscoveryMode,
) -> anyhow::Result<RefreshStats> {
    let root_id = upsert_root(pool, root).await?;
    mark_dirty_inner(pool, root_id, root, options, IndexScope::Root, mode).await
}

async fn mark_dirty_inner(
    pool: &Pool,
    root_id: Uuid,
    root: &CodeRootSpec,
    options: &IndexOptions,
    scope: IndexScope,
    mode: DiscoveryMode,
) -> anyhow::Result<RefreshStats> {
    let (candidates, deletion_scope) = discover_scope_candidates(root, &options.walk, scope)?;
    let mut stats = RefreshStats {
        files_seen: candidates.len(),
        ..RefreshStats::default()
    };
    let mut seen_paths = HashSet::new();
    for candidate in candidates {
        let relative_path = relative_path(&root.path, &candidate.path)?;
        seen_paths.insert(relative_path.clone());
        if mark_candidate_pending(pool, root_id, &relative_path, &candidate, mode).await? {
            stats.files_marked_pending += 1;
        }
    }
    stats.files_deleted = mark_deleted_files(pool, root_id, &seen_paths, deletion_scope).await?;
    sqlx::query("UPDATE code_roots SET last_scan_at = NOW(), updated_at = NOW() WHERE id = $1")
        .bind(root_id)
        .execute(pool)
        .await?;
    Ok(stats)
}

pub async fn index_pending_root(
    pool: &Pool,
    root: &CodeRootSpec,
    options: &IndexOptions,
    limit: i64,
) -> anyhow::Result<IndexStats> {
    let root_id = upsert_root(pool, root).await?;
    let pending = load_pending_files(pool, root_id, limit).await?;
    if pending.is_empty() {
        return Ok(IndexStats::default());
    }
    let job_id = start_job(pool, root_id, options.trigger, None).await?;
    let result = index_pending_files(pool, root_id, root, &options.walk, pending).await;
    finish_job(pool, job_id, &result).await?;
    result
}

async fn index_pending_files(
    pool: &Pool,
    root_id: Uuid,
    root: &CodeRootSpec,
    walk: &SourceWalkOptions,
    pending: Vec<PendingFile>,
) -> anyhow::Result<IndexStats> {
    let mut parser = SourceParser::default();
    let mut stats = IndexStats {
        files_seen: pending.len(),
        ..IndexStats::default()
    };
    for file in pending {
        match index_pending_file(pool, root_id, root, walk, file.id, &file.path, &mut parser).await
        {
            Ok(FileIndexOutcome::Indexed { symbols }) => {
                stats.files_indexed += 1;
                stats.symbols_indexed += symbols;
            }
            Ok(FileIndexOutcome::SkippedUnchanged) => {
                stats.files_skipped_unchanged += 1;
            }
            Err(err) => {
                tracing::warn!(path = %file.path, %err, "code-intel file index failed");
                stats.files_failed += 1;
                mark_file_failed(pool, root_id, &file.path, err.to_string()).await?;
            }
        }
    }
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
    if let IndexScope::Path(path) = &scope {
        if is_ignored_path_in_root(&root.path, path)? {
            return Ok((Vec::new(), deletion_scope_for_path(&root.path, path)?));
        }
    }
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
            let candidates = source_file_candidate_in_root(&root.path, &path, walk)?
                .into_iter()
                .collect();
            Ok((candidates, DeletionScope::Exact(relative)))
        }
        IndexScope::Path(path) => {
            let relative = relative_path(&root.path, &path)?;
            Ok((Vec::new(), DeletionScope::MissingPath(relative)))
        }
    }
}

fn deletion_scope_for_path(root: &Path, path: &Path) -> anyhow::Result<DeletionScope> {
    let relative = relative_path(root, path)?;
    if path.is_file() {
        Ok(DeletionScope::Exact(relative))
    } else if path.is_dir() {
        if relative.is_empty() {
            Ok(DeletionScope::Root)
        } else {
            Ok(DeletionScope::Prefix(format!(
                "{}/",
                relative.trim_end_matches('/')
            )))
        }
    } else {
        Ok(DeletionScope::MissingPath(relative))
    }
}

enum FileIndexOutcome {
    Indexed { symbols: usize },
    SkippedUnchanged,
}

struct PendingFile {
    id: Uuid,
    path: String,
}

async fn load_pending_files(
    pool: &Pool,
    root_id: Uuid,
    limit: i64,
) -> anyhow::Result<Vec<PendingFile>> {
    let rows = sqlx::query(
        "SELECT id, path \
           FROM code_files \
          WHERE root_id = $1 \
            AND deleted_at IS NULL \
            AND parse_status = 'pending' \
          ORDER BY updated_at ASC, path ASC \
          LIMIT $2",
    )
    .bind(root_id)
    .bind(limit.max(1))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| PendingFile {
            id: row.get("id"),
            path: row.get("path"),
        })
        .collect())
}

async fn index_pending_file(
    pool: &Pool,
    root_id: Uuid,
    root: &CodeRootSpec,
    walk: &SourceWalkOptions,
    file_id: Uuid,
    relative_path: &str,
    parser: &mut SourceParser,
) -> anyhow::Result<FileIndexOutcome> {
    let absolute_path = root.path.join(relative_path);
    if !absolute_path.is_file() {
        mark_file_deleted(pool, file_id).await?;
        return Ok(FileIndexOutcome::SkippedUnchanged);
    }
    let Some(candidate) = source_file_candidate_in_root(&root.path, &absolute_path, walk)? else {
        mark_file_unsupported(pool, file_id).await?;
        return Ok(FileIndexOutcome::SkippedUnchanged);
    };
    let source = fs::read_to_string(&absolute_path)
        .with_context(|| format!("read {}", absolute_path.display()))?;
    let content_hash = hash_bytes(source.as_bytes());
    let language = candidate.language;
    let metadata = fs::metadata(&absolute_path)
        .with_context(|| format!("stat {}", absolute_path.display()))?;
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

fn is_generated_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_ignored_dir_name)
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
