use super::*;

#[derive(Debug, Clone)]
pub(super) struct EmbeddingSource {
    pub(super) source_family: SourceFamily,
    pub(super) source_kind: SourceKind,
    pub(super) source_key: String,
    pub(super) session_uuid: Uuid,
    pub(super) byte_offset: Option<i64>,
    pub(super) block_ord: Option<i32>,
    pub(super) turn_id: Option<i64>,
    pub(super) operation_ord: Option<i32>,
    pub(super) repo_name: Option<String>,
    pub(super) content_hash: String,
    pub(super) text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceFamily {
    EventBlock,
    OperationCall,
    OperationResult,
}

impl SourceFamily {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::EventBlock => "event_block",
            Self::OperationCall => "operation_call",
            Self::OperationResult => "operation_result",
        }
    }

    pub(super) fn from_db(raw: &str) -> Result<Self, RetrievalError> {
        match raw {
            "event_block" => Ok(Self::EventBlock),
            "operation_call" => Ok(Self::OperationCall),
            "operation_result" => Ok(Self::OperationResult),
            other => Err(RetrievalError::internal(anyhow!(
                "unknown retrieval source family: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
struct BackfillRun {
    id: Uuid,
    generation: i64,
    source_family: SourceFamily,
    scope_repo: Option<String>,
    scope_session_uuid: Option<Uuid>,
    force: bool,
    cursor_session_uuid: Option<Uuid>,
    cursor_byte_offset: Option<i64>,
    cursor_block_ord: Option<i32>,
    cursor_turn_id: Option<i64>,
    cursor_operation_ord: Option<i32>,
    started_at: DateTime<Utc>,
}

pub(super) struct PendingSourceRow {
    pub(super) source_family: SourceFamily,
    pub(super) source_key: String,
    pub(super) session_uuid: Uuid,
    pub(super) byte_offset: Option<i64>,
    pub(super) block_ord: Option<i32>,
    pub(super) turn_id: Option<i64>,
    pub(super) operation_ord: Option<i32>,
}

pub(super) async fn next_backfill_generation(
    state: &RetrievalState,
) -> Result<i64, RetrievalError> {
    let generation =
        sqlx::query_scalar("SELECT nextval('retrieval_embedding_backfill_generation_seq')::BIGINT")
            .fetch_one(&state.pool)
            .await?;
    Ok(generation)
}

pub(super) async fn start_backfill_runs(
    state: &RetrievalState,
    generation: i64,
    repo: Option<&str>,
    session_uuid: Option<Uuid>,
    force: bool,
) -> Result<usize, RetrievalError> {
    let families = [
        SourceFamily::EventBlock,
        SourceFamily::OperationCall,
        SourceFamily::OperationResult,
    ];
    let mut started = 0;
    for family in families {
        sqlx::query(
            "INSERT INTO retrieval_embedding_backfills \
                (generation, source_family, scope_repo, scope_session_uuid, force, status, updated_at) \
             VALUES ($1, $2, $3, $4, $5, 'running', NOW())",
        )
        .bind(generation)
        .bind(family.as_str())
        .bind(repo)
        .bind(session_uuid)
        .bind(force)
        .execute(&state.pool)
        .await?;
        started += 1;
    }
    Ok(started)
}

pub(super) async fn advance_backfill_runs(
    state: &RetrievalState,
    limit: i64,
) -> Result<(), RetrievalError> {
    let runs = load_running_backfill_runs(state).await?;
    if runs.is_empty() {
        return Ok(());
    }
    let limit = limit.clamp(1, MAX_REINDEX_LIMIT);
    for run in runs {
        if let Err(err) = advance_backfill_run(state, &run, limit).await {
            mark_backfill_failed(state, run.id, &err.to_string()).await?;
            return Err(err);
        }
    }
    Ok(())
}

async fn load_running_backfill_runs(
    state: &RetrievalState,
) -> Result<Vec<BackfillRun>, RetrievalError> {
    let rows = sqlx::query(
        "SELECT id, generation, source_family, scope_repo, scope_session_uuid, force, \
                cursor_session_uuid, cursor_byte_offset, cursor_block_ord, cursor_turn_id, \
                cursor_operation_ord, started_at \
           FROM retrieval_embedding_backfills \
          WHERE status = 'running' \
          ORDER BY updated_at ASC, id ASC \
          LIMIT 3",
    )
    .fetch_all(&state.pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let family: String = row.try_get("source_family")?;
            Ok(BackfillRun {
                id: row.try_get("id")?,
                generation: row.try_get("generation")?,
                source_family: SourceFamily::from_db(&family)?,
                scope_repo: row.try_get("scope_repo")?,
                scope_session_uuid: row.try_get("scope_session_uuid")?,
                force: row.try_get("force")?,
                cursor_session_uuid: row.try_get("cursor_session_uuid")?,
                cursor_byte_offset: row.try_get("cursor_byte_offset")?,
                cursor_block_ord: row.try_get("cursor_block_ord")?,
                cursor_turn_id: row.try_get("cursor_turn_id")?,
                cursor_operation_ord: row.try_get("cursor_operation_ord")?,
                started_at: row.try_get("started_at")?,
            })
        })
        .collect()
}

async fn advance_backfill_run(
    state: &RetrievalState,
    run: &BackfillRun,
    limit: i64,
) -> Result<(), RetrievalError> {
    let sources = load_backfill_sources(state, run, limit).await?;
    let mut marked_pending = 0_i64;
    for source in &sources {
        if upsert_source_seen(state, source, run.generation, run.force).await? {
            marked_pending += 1;
        }
    }

    if let Some(last) = sources.last() {
        sqlx::query(
            "UPDATE retrieval_embedding_backfills \
                SET cursor_session_uuid = $2, \
                    cursor_byte_offset = $3, \
                    cursor_block_ord = $4, \
                    cursor_turn_id = $5, \
                    cursor_operation_ord = $6, \
                    rows_seen = rows_seen + $7, \
                    rows_marked_pending = rows_marked_pending + $8, \
                    updated_at = NOW() \
              WHERE id = $1",
        )
        .bind(run.id)
        .bind(last.session_uuid)
        .bind(last.byte_offset)
        .bind(last.block_ord)
        .bind(last.turn_id)
        .bind(last.operation_ord)
        .bind(sources.len() as i64)
        .bind(marked_pending)
        .execute(&state.pool)
        .await?;
    }

    if sources.len() < limit as usize {
        delete_stale_sources_for_completed_run(state, run).await?;
        sqlx::query(
            "UPDATE retrieval_embedding_backfills \
                SET status = 'complete', finished_at = NOW(), updated_at = NOW() \
              WHERE id = $1",
        )
        .bind(run.id)
        .execute(&state.pool)
        .await?;
    }
    Ok(())
}

async fn mark_backfill_failed(
    state: &RetrievalState,
    id: Uuid,
    error: &str,
) -> Result<(), RetrievalError> {
    sqlx::query(
        "UPDATE retrieval_embedding_backfills \
            SET status = 'failed', last_error = $2, finished_at = NOW(), updated_at = NOW() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(error)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn load_backfill_sources(
    state: &RetrievalState,
    run: &BackfillRun,
    limit: i64,
) -> Result<Vec<EmbeddingSource>, RetrievalError> {
    match run.source_family {
        SourceFamily::EventBlock => load_event_backfill_sources(state, run, limit).await,
        SourceFamily::OperationCall => {
            load_operation_backfill_sources(state, run, false, limit).await
        }
        SourceFamily::OperationResult => {
            load_operation_backfill_sources(state, run, true, limit).await
        }
    }
}

async fn load_event_backfill_sources(
    state: &RetrievalState,
    run: &BackfillRun,
    limit: i64,
) -> Result<Vec<EmbeddingSource>, RetrievalError> {
    let rows = sqlx::query(
        "SELECT \
            CASE \
              WHEN e.speaker = 'assistant' AND b.kind = 'text' THEN 'assistant_text' \
              WHEN e.speaker = 'user' AND b.kind = 'text' THEN 'user_prompt' \
              WHEN e.speaker = 'summary' AND b.kind = 'text' THEN 'summary' \
              WHEN b.kind = 'tool_result' AND COALESCE(b.is_error, FALSE) THEN 'tool_error' \
              ELSE NULL \
            END AS source_kind, \
            ('event:' || e.session_uuid::TEXT || ':' || e.byte_offset::TEXT || ':' || b.ord::TEXT) AS source_key, \
            e.session_uuid, e.byte_offset, b.ord AS block_ord, tt.turn_id, NULL::INT AS operation_ord, \
            COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) AS repo_name, \
            CASE WHEN b.kind = 'tool_result' THEN left(b.text, 1000) ELSE b.text END AS text \
           FROM event_blocks b \
           JOIN events e ON e.session_uuid = b.session_uuid AND e.byte_offset = b.byte_offset \
           JOIN claude_sessions cs ON cs.session_uuid = e.session_uuid \
           LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
           LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
           LEFT JOIN LATERAL ( \
              SELECT turn_id \
                FROM timeline_turns tt \
               WHERE tt.session_uuid = e.session_uuid \
                 AND tt.start_timestamp <= e.timestamp \
                 AND tt.end_timestamp >= e.timestamp \
               ORDER BY tt.duration_ms ASC, tt.turn_id ASC \
               LIMIT 1 \
           ) tt ON TRUE \
          WHERE b.text IS NOT NULL \
            AND length(trim(b.text)) > 0 \
            AND ( \
                (e.speaker IN ('assistant', 'user', 'summary') AND b.kind = 'text') \
                OR (b.kind = 'tool_result' AND COALESCE(b.is_error, FALSE)) \
            ) \
            AND ($1::UUID IS NULL OR e.session_uuid = $1) \
            AND ($2::TEXT IS NULL OR COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) = $2) \
            AND ( \
                $3::UUID IS NULL \
                OR e.session_uuid > $3 \
                OR (e.session_uuid = $3 AND e.byte_offset > $4) \
                OR (e.session_uuid = $3 AND e.byte_offset = $4 AND b.ord > $5) \
            ) \
          ORDER BY e.session_uuid ASC, e.byte_offset ASC, b.ord ASC \
          LIMIT $6",
    )
    .bind(run.scope_session_uuid)
    .bind(run.scope_repo.as_deref())
    .bind(run.cursor_session_uuid)
    .bind(run.cursor_byte_offset)
    .bind(run.cursor_block_ord)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;

    source_rows_from_db(rows, SourceFamily::EventBlock)
}

async fn load_operation_backfill_sources(
    state: &RetrievalState,
    run: &BackfillRun,
    result_sources: bool,
    limit: i64,
) -> Result<Vec<EmbeddingSource>, RetrievalError> {
    let sql = if result_sources {
        "SELECT \
            CASE WHEN o.result_is_error OR o.is_error THEN 'tool_error' ELSE 'tool_result' END AS source_kind, \
            ('operation:' || o.session_uuid::TEXT || ':' || o.turn_id::TEXT || ':' || o.operation_ord::TEXT || ':result') AS source_key, \
            o.session_uuid, NULL::BIGINT AS byte_offset, NULL::INT AS block_ord, o.turn_id, o.operation_ord, \
            COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) AS repo_name, \
            CASE WHEN o.result_is_error OR o.is_error \
                 THEN left(concat_ws(' ', o.name, o.result_content, o.result_payload::TEXT), 1000) \
                 ELSE concat_ws(' ', o.name, o.result_content, o.result_payload::TEXT) END AS text \
           FROM timeline_operations o \
           JOIN claude_sessions cs ON cs.session_uuid = o.session_uuid \
           LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
           LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
          WHERE (o.result_content IS NOT NULL OR o.result_payload IS NOT NULL OR o.result_is_error OR o.is_error) \
            AND length(trim(concat_ws(' ', o.name, o.result_content, o.result_payload::TEXT))) > 0 \
            AND (o.result_is_error OR o.is_error OR o.name = 'agent') \
            AND ($1::UUID IS NULL OR o.session_uuid = $1) \
            AND ($2::TEXT IS NULL OR COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) = $2) \
            AND ( \
                $3::UUID IS NULL \
                OR o.session_uuid > $3 \
                OR (o.session_uuid = $3 AND o.turn_id > $4) \
                OR (o.session_uuid = $3 AND o.turn_id = $4 AND o.operation_ord > $5) \
            ) \
          ORDER BY o.session_uuid ASC, o.turn_id ASC, o.operation_ord ASC \
          LIMIT $6"
    } else {
        "SELECT \
            'tool_call' AS source_kind, \
            ('operation:' || o.session_uuid::TEXT || ':' || o.turn_id::TEXT || ':' || o.operation_ord::TEXT || ':call') AS source_key, \
            o.session_uuid, NULL::BIGINT AS byte_offset, NULL::INT AS block_ord, o.turn_id, o.operation_ord, \
            COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) AS repo_name, \
            concat_ws(' ', o.name, o.raw_name, o.operation_type, o.operation_category, left(o.input::TEXT, 300)) AS text \
           FROM timeline_operations o \
           JOIN claude_sessions cs ON cs.session_uuid = o.session_uuid \
           LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
           LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
          WHERE o.input IS NOT NULL \
            AND length(trim(concat_ws(' ', o.name, o.raw_name, o.operation_type, o.operation_category, o.input::TEXT))) > 0 \
            AND ($1::UUID IS NULL OR o.session_uuid = $1) \
            AND ($2::TEXT IS NULL OR COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) = $2) \
            AND ( \
                $3::UUID IS NULL \
                OR o.session_uuid > $3 \
                OR (o.session_uuid = $3 AND o.turn_id > $4) \
                OR (o.session_uuid = $3 AND o.turn_id = $4 AND o.operation_ord > $5) \
            ) \
          ORDER BY o.session_uuid ASC, o.turn_id ASC, o.operation_ord ASC \
          LIMIT $6"
    };
    let rows = sqlx::query(sql)
        .bind(run.scope_session_uuid)
        .bind(run.scope_repo.as_deref())
        .bind(run.cursor_session_uuid)
        .bind(run.cursor_turn_id)
        .bind(run.cursor_operation_ord)
        .bind(limit)
        .fetch_all(&state.pool)
        .await?;
    source_rows_from_db(
        rows,
        if result_sources {
            SourceFamily::OperationResult
        } else {
            SourceFamily::OperationCall
        },
    )
}

fn source_rows_from_db(
    rows: Vec<sqlx::postgres::PgRow>,
    source_family: SourceFamily,
) -> Result<Vec<EmbeddingSource>, RetrievalError> {
    let mut sources = Vec::new();
    for row in rows {
        if let Some(source) = source_from_row(row, source_family)? {
            sources.push(source);
        }
    }
    Ok(sources)
}

pub(super) fn source_from_row(
    row: sqlx::postgres::PgRow,
    source_family: SourceFamily,
) -> Result<Option<EmbeddingSource>, RetrievalError> {
    let kind: String = match row.try_get("source_kind") {
        Ok(kind) => kind,
        Err(_) => return Ok(None),
    };
    let text: String = match row.try_get("text") {
        Ok(text) => text,
        Err(_) => return Ok(None),
    };
    let Some(source_kind) = source_kind_from_db(&kind) else {
        return Ok(None);
    };
    Ok(Some(EmbeddingSource {
        source_family,
        source_kind,
        source_key: row.try_get("source_key")?,
        session_uuid: row.try_get("session_uuid")?,
        byte_offset: row.try_get("byte_offset").ok().flatten(),
        block_ord: row.try_get("block_ord").ok().flatten(),
        turn_id: row.try_get("turn_id").ok().flatten(),
        operation_ord: row.try_get("operation_ord").ok().flatten(),
        repo_name: row.try_get("repo_name").ok().flatten(),
        content_hash: hash_text(&text),
        text,
    }))
}

fn source_kind_from_db(raw: &str) -> Option<SourceKind> {
    match raw {
        "assistant_text" => Some(SourceKind::AssistantText),
        "user_prompt" => Some(SourceKind::UserPrompt),
        "summary" => Some(SourceKind::Summary),
        "tool_call" => Some(SourceKind::ToolCall),
        "tool_result" => Some(SourceKind::ToolResult),
        "tool_error" => Some(SourceKind::ToolError),
        "turn_digest" => Some(SourceKind::TurnDigest),
        _ => None,
    }
}

async fn upsert_source_seen(
    state: &RetrievalState,
    source: &EmbeddingSource,
    generation: i64,
    force: bool,
) -> Result<bool, RetrievalError> {
    let existing = sqlx::query(
        "SELECT s.content_hash, s.index_status, s.deleted_at, \
                EXISTS ( \
                    SELECT 1 \
                      FROM retrieval_embeddings re \
                     WHERE re.embedding_model = $2 \
                       AND re.embedding_dimensions = $3 \
                       AND re.source_key = s.source_key \
                       AND re.content_hash = s.content_hash \
                ) AS has_current_embedding \
           FROM retrieval_embedding_sources s \
          WHERE s.source_key = $1",
    )
    .bind(&source.source_key)
    .bind(&state.config.embedding_model)
    .bind(state.config.embedding_dimensions)
    .fetch_optional(&state.pool)
    .await?;

    let should_pending = match existing {
        None => true,
        Some(row) => {
            let content_hash: String = row.try_get("content_hash")?;
            let index_status: String = row.try_get("index_status")?;
            let deleted_at: Option<DateTime<Utc>> = row.try_get("deleted_at")?;
            let has_current_embedding: bool = row.try_get("has_current_embedding")?;
            force
                || deleted_at.is_some()
                || index_status != "indexed"
                || content_hash != source.content_hash
                || !has_current_embedding
        }
    };
    let status = if should_pending { "pending" } else { "indexed" };

    sqlx::query(
        "INSERT INTO retrieval_embedding_sources \
            (source_family, source_kind, source_key, session_uuid, byte_offset, block_ord, \
             turn_id, operation_ord, repo_name, content_hash, last_seen_generation, \
             index_status, index_error, last_seen_at, dirty_at, deleted_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, NULL, NOW(), NOW(), NULL, NOW()) \
         ON CONFLICT (source_key) DO UPDATE SET \
             source_family = EXCLUDED.source_family, \
             source_kind = EXCLUDED.source_kind, \
             session_uuid = EXCLUDED.session_uuid, \
             byte_offset = EXCLUDED.byte_offset, \
             block_ord = EXCLUDED.block_ord, \
             turn_id = EXCLUDED.turn_id, \
             operation_ord = EXCLUDED.operation_ord, \
             repo_name = EXCLUDED.repo_name, \
             content_hash = EXCLUDED.content_hash, \
             last_seen_generation = EXCLUDED.last_seen_generation, \
             index_status = EXCLUDED.index_status, \
             index_error = CASE WHEN $13::BOOLEAN THEN NULL ELSE retrieval_embedding_sources.index_error END, \
             last_seen_at = NOW(), \
             dirty_at = CASE WHEN $13::BOOLEAN THEN NOW() ELSE retrieval_embedding_sources.dirty_at END, \
             deleted_at = NULL, \
             updated_at = NOW()",
    )
    .bind(source.source_family.as_str())
    .bind(source.source_kind.as_str())
    .bind(&source.source_key)
    .bind(source.session_uuid)
    .bind(source.byte_offset)
    .bind(source.block_ord)
    .bind(source.turn_id)
    .bind(source.operation_ord)
    .bind(source.repo_name.as_deref())
    .bind(&source.content_hash)
    .bind(generation)
    .bind(status)
    .bind(should_pending)
    .execute(&state.pool)
    .await?;

    Ok(should_pending)
}

async fn delete_stale_sources_for_completed_run(
    state: &RetrievalState,
    run: &BackfillRun,
) -> Result<i64, RetrievalError> {
    let deleted = sqlx::query_scalar(
        "WITH stale AS ( \
             SELECT source_key \
               FROM retrieval_embedding_sources \
              WHERE source_family = $1 \
                AND deleted_at IS NULL \
                AND COALESCE(last_seen_generation, 0) < $2 \
                AND updated_at < $3 \
                AND ($4::UUID IS NULL OR session_uuid = $4) \
                AND ($5::TEXT IS NULL OR repo_name = $5) \
        ), removed_embeddings AS ( \
             DELETE FROM retrieval_embeddings re \
              USING stale \
              WHERE re.source_key = stale.source_key \
              RETURNING re.source_key \
        ), removed_sources AS ( \
             UPDATE retrieval_embedding_sources s \
                SET index_status = 'deleted', deleted_at = NOW(), updated_at = NOW() \
               FROM stale \
              WHERE s.source_key = stale.source_key \
              RETURNING s.id \
        ) \
        SELECT COUNT(*)::BIGINT FROM removed_sources",
    )
    .bind(run.source_family.as_str())
    .bind(run.generation)
    .bind(run.started_at)
    .bind(run.scope_session_uuid)
    .bind(run.scope_repo.as_deref())
    .fetch_one(&state.pool)
    .await?;
    Ok(deleted)
}
