use super::*;

pub(super) async fn upsert_root(pool: &Pool, root: &CodeRootSpec) -> anyhow::Result<Uuid> {
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

pub(super) async fn mark_candidate_pending(
    pool: &Pool,
    root_id: Uuid,
    relative_path: &str,
    candidate: &SourceFileCandidate,
    mode: DiscoveryMode,
) -> anyhow::Result<bool> {
    let metadata = fs::metadata(&candidate.path)
        .with_context(|| format!("stat {}", candidate.path.display()))?;
    let mtime = metadata.modified().ok().map(DateTime::<Utc>::from);
    let size_bytes = candidate.size_bytes as i64;
    let language = candidate.language.as_str();
    let row = match mode {
        DiscoveryMode::ForcePending => {
            sqlx::query(
                "INSERT INTO code_files \
                 (root_id, path, language, size_bytes, mtime, parse_status, \
                  parse_error_count, metadata, deleted_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, 'pending', 0, \
                         jsonb_build_object('indexer_version', $6::int), NULL, NOW()) \
                 ON CONFLICT (root_id, path) DO UPDATE SET \
                   language = EXCLUDED.language, \
                   size_bytes = EXCLUDED.size_bytes, \
                   mtime = EXCLUDED.mtime, \
                   parse_status = 'pending', \
                   parse_error_count = 0, \
                   metadata = code_files.metadata || jsonb_build_object('indexer_version', $6::int), \
                   deleted_at = NULL, \
                   updated_at = NOW() \
                 RETURNING id",
            )
            .bind(root_id)
            .bind(relative_path)
            .bind(language)
            .bind(size_bytes)
            .bind(mtime)
            .bind(INDEXER_VERSION)
            .fetch_optional(pool)
            .await?
        }
        DiscoveryMode::DetectChanges => {
            sqlx::query(
                "INSERT INTO code_files \
                 (root_id, path, language, size_bytes, mtime, parse_status, \
                  parse_error_count, metadata, deleted_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, 'pending', 0, \
                         jsonb_build_object('indexer_version', $6::int), NULL, NOW()) \
                 ON CONFLICT (root_id, path) DO UPDATE SET \
                   language = EXCLUDED.language, \
                   size_bytes = EXCLUDED.size_bytes, \
                   mtime = EXCLUDED.mtime, \
                   parse_status = 'pending', \
                   parse_error_count = 0, \
                   metadata = code_files.metadata || jsonb_build_object('indexer_version', $6::int), \
                   deleted_at = NULL, \
                   updated_at = NOW() \
                 WHERE code_files.deleted_at IS NOT NULL \
                    OR code_files.parse_status = 'unsupported' \
                    OR code_files.language IS DISTINCT FROM EXCLUDED.language \
                    OR code_files.size_bytes IS DISTINCT FROM EXCLUDED.size_bytes \
                    OR code_files.mtime IS DISTINCT FROM EXCLUDED.mtime \
                    OR code_files.metadata->>'indexer_version' IS DISTINCT FROM $6::text \
                 RETURNING id",
            )
            .bind(root_id)
            .bind(relative_path)
            .bind(language)
            .bind(size_bytes)
            .bind(mtime)
            .bind(INDEXER_VERSION)
            .fetch_optional(pool)
            .await?
        }
    };
    Ok(row.is_some())
}

pub(super) async fn start_job(
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

pub(super) async fn finish_job(
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

#[allow(clippy::too_many_arguments)]
pub(super) async fn upsert_file(
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

pub(super) async fn replace_symbols(
    pool: &Pool,
    root_id: Uuid,
    file_id: Uuid,
    symbols: &[ExtractedSymbol],
    references: &[ExtractedReference],
) -> anyhow::Result<()> {
    clear_file_facts(pool, file_id).await?;
    for symbol in symbols {
        insert_symbol(pool, root_id, file_id, symbol).await?;
    }
    for reference in references {
        insert_reference(pool, root_id, file_id, reference).await?;
    }
    Ok(())
}

async fn clear_file_facts(pool: &Pool, file_id: Uuid) -> anyhow::Result<()> {
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

pub(super) async fn mark_file_failed(
    pool: &Pool,
    root_id: Uuid,
    relative_path: &str,
    error: String,
) -> anyhow::Result<()> {
    if let Some(file_id) =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM code_files WHERE root_id = $1 AND path = $2")
            .bind(root_id)
            .bind(relative_path)
            .fetch_optional(pool)
            .await?
    {
        clear_file_facts(pool, file_id).await?;
    }
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

pub(super) async fn mark_file_deleted(pool: &Pool, file_id: Uuid) -> anyhow::Result<()> {
    clear_file_facts(pool, file_id).await?;
    sqlx::query(
        "UPDATE code_files SET parse_status = 'deleted', deleted_at = NOW(), updated_at = NOW() \
         WHERE id = $1",
    )
    .bind(file_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub(super) async fn mark_file_unsupported(pool: &Pool, file_id: Uuid) -> anyhow::Result<()> {
    clear_file_facts(pool, file_id).await?;
    sqlx::query(
        "UPDATE code_files SET \
           parse_status = 'unsupported', \
           parse_error_count = 0, \
           indexed_at = NOW(), \
           deleted_at = NULL, \
           updated_at = NOW() \
         WHERE id = $1",
    )
    .bind(file_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub(super) async fn mark_deleted_files(
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
        mark_file_deleted(pool, file_id).await?;
        deleted += 1;
    }
    Ok(deleted)
}
