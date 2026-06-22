use super::*;

#[derive(Debug, Deserialize)]
pub(super) struct ReindexRequest {
    pub(super) repo: Option<String>,
    pub(super) agent_session_uuid: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub(super) struct ReindexResponse {
    generation: i64,
    backfills_started: usize,
    sources_seen: usize,
    sources_marked_pending: usize,
    sources_deleted: usize,
    pending_sources: i64,
    vector: VectorCapabilities,
    embedding_model: String,
    embedding_dimensions: i32,
}

#[derive(Debug, Deserialize)]
pub(super) struct ResetRequest {
    #[serde(default)]
    pub(super) confirm: bool,
    pub(super) reschedule: Option<bool>,
}

#[derive(Debug, Serialize)]
pub(super) struct ResetResponse {
    embeddings_deleted: i64,
    sources_deleted: i64,
    backfills_deleted: i64,
    generation: Option<i64>,
    backfills_started: usize,
    pending_sources: i64,
    vector: VectorCapabilities,
    embedding_model: String,
    embedding_dimensions: i32,
}

#[derive(Debug, Serialize)]
pub(super) struct IndexStatusResponse {
    pending_sources: i64,
    indexed_sources: i64,
    failed_sources: i64,
    deleted_sources: i64,
    running_backfills: i64,
    failed_backfills: i64,
    latest_backfill_generation: Option<i64>,
    backfill_rows_seen: i64,
    backfill_rows_marked_pending: i64,
    embedding_count: i64,
    latest_indexed_at: Option<DateTime<Utc>>,
    vector: VectorCapabilities,
    embedding_model: String,
    embedding_dimensions: i32,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct EmbeddingIndexStats {
    pub(super) sources_seen: usize,
    pub(super) embedded: usize,
    pub(super) skipped: usize,
    pub(super) failed: usize,
}

pub(super) async fn reindex_route(
    State(state): State<Arc<RetrievalState>>,
    Json(request): Json<ReindexRequest>,
) -> Result<Json<ReindexResponse>, RetrievalError> {
    let response = reindex_inner(&state, request).await?;
    Ok(Json(response))
}

pub(super) async fn reindex_inner(
    state: &RetrievalState,
    request: ReindexRequest,
) -> Result<ReindexResponse, RetrievalError> {
    let vector = state.refresh_vector_capabilities().await?;
    let _guard = state.index_lock.lock().await;
    let generation = next_backfill_generation(state).await?;
    let backfills_started = start_backfill_runs(
        state,
        generation,
        request.repo.as_deref(),
        request.agent_session_uuid,
        false,
    )
    .await?;
    let status = load_index_status_inner(state, vector).await?;
    Ok(ReindexResponse {
        generation,
        backfills_started,
        sources_seen: 0,
        sources_marked_pending: 0,
        sources_deleted: 0,
        pending_sources: status.pending_sources,
        vector,
        embedding_model: state.config.embedding_model.clone(),
        embedding_dimensions: state.config.embedding_dimensions,
    })
}

pub(super) async fn reset_route(
    State(state): State<Arc<RetrievalState>>,
    Json(request): Json<ResetRequest>,
) -> Result<Json<ResetResponse>, RetrievalError> {
    Ok(Json(reset_inner(&state, request).await?))
}

/// Wipe the derived semantic-index state and (by default) reschedule a full
/// rebuild. Runs under `index_lock` so it serializes with the background indexer
/// rather than truncating tables out from under an in-flight backfill. Transcript
/// text is untouched; only embeddings and the backfill/source queue are reset.
pub(super) async fn reset_inner(
    state: &RetrievalState,
    request: ResetRequest,
) -> Result<ResetResponse, RetrievalError> {
    if !request.confirm {
        return Err(RetrievalError::bad_request(
            "index reset requires {\"confirm\": true}",
        ));
    }
    let vector = state.refresh_vector_capabilities().await?;
    // Serialize against the crawler: the background indexer holds this same lock
    // while advancing backfills, so the wipe waits for an in-flight pass to finish
    // and blocks the next one until the rebuild is scheduled.
    let _guard = state.index_lock.lock().await;

    let embeddings_deleted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM retrieval_embeddings")
        .fetch_one(&state.pool)
        .await?;
    let sources_deleted: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM retrieval_embedding_sources")
            .fetch_one(&state.pool)
            .await?;
    let backfills_deleted: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM retrieval_embedding_backfills")
            .fetch_one(&state.pool)
            .await?;
    sqlx::query(
        "TRUNCATE retrieval_embeddings, retrieval_embedding_sources, \
                  retrieval_embedding_backfills RESTART IDENTITY",
    )
    .execute(&state.pool)
    .await?;

    let (generation, backfills_started) = if request.reschedule.unwrap_or(true) {
        let generation = next_backfill_generation(state).await?;
        let started = start_backfill_runs(state, generation, None, None, false).await?;
        tracing::info!(
            generation,
            backfills_started = started,
            embeddings_deleted,
            sources_deleted,
            "reset retrieval semantic index and rescheduled full backfill"
        );
        (Some(generation), started)
    } else {
        tracing::info!(
            embeddings_deleted,
            sources_deleted,
            "reset retrieval semantic index without rescheduling"
        );
        (None, 0)
    };

    let status = load_index_status_inner(state, vector).await?;
    Ok(ResetResponse {
        embeddings_deleted,
        sources_deleted,
        backfills_deleted,
        generation,
        backfills_started,
        pending_sources: status.pending_sources,
        vector,
        embedding_model: state.config.embedding_model.clone(),
        embedding_dimensions: state.config.embedding_dimensions,
    })
}

pub(super) async fn bootstrap_index_if_empty(
    state: &RetrievalState,
) -> Result<bool, RetrievalError> {
    let _guard = state.index_lock.lock().await;
    let (source_count, backfill_count): (i64, i64) = sqlx::query_as(
        "SELECT \
            (SELECT COUNT(*) FROM retrieval_embedding_sources)::BIGINT, \
            (SELECT COUNT(*) FROM retrieval_embedding_backfills)::BIGINT",
    )
    .fetch_one(&state.pool)
    .await?;
    if source_count > 0 || backfill_count > 0 {
        return Ok(false);
    }
    let generation = next_backfill_generation(state).await?;
    let started = start_backfill_runs(state, generation, None, None, false).await?;
    tracing::info!(
        generation,
        backfills_started = started,
        "scheduled initial retrieval semantic index backfill"
    );
    Ok(started > 0)
}

pub(super) async fn index_status_route(
    State(state): State<Arc<RetrievalState>>,
) -> Result<Json<IndexStatusResponse>, RetrievalError> {
    let vector = *state.vector_capabilities.read().await;
    Ok(Json(load_index_status_inner(&state, vector).await?))
}

pub(super) async fn pending_source_count(state: &RetrievalState) -> Result<i64, RetrievalError> {
    let count = sqlx::query_scalar(
        "SELECT COUNT(*) \
           FROM retrieval_embedding_sources \
          WHERE index_status = 'pending' \
            AND deleted_at IS NULL",
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(count)
}

pub(super) async fn index_has_work(state: &RetrievalState) -> Result<bool, RetrievalError> {
    let has_work = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM retrieval_embedding_sources \
              WHERE index_status = 'pending' AND deleted_at IS NULL \
             UNION ALL \
             SELECT 1 FROM retrieval_embedding_backfills \
              WHERE status = 'running' \
         )",
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(has_work)
}

pub(super) async fn run_retrieval_indexer_once(
    state: &RetrievalState,
    limit: i64,
) -> Result<EmbeddingIndexStats, RetrievalError> {
    let _guard = state.index_lock.lock().await;
    state.refresh_vector_capabilities().await?;
    advance_backfill_runs(state, DEFAULT_REINDEX_LIMIT).await?;
    index_pending_embeddings(state, limit).await
}

pub(super) async fn index_pending_embeddings(
    state: &RetrievalState,
    limit: i64,
) -> Result<EmbeddingIndexStats, RetrievalError> {
    let embedder = state.embedding_client();
    let vector = *state.vector_capabilities.read().await;
    let limit = limit.clamp(1, MAX_REINDEX_LIMIT);
    let sources = load_pending_embedding_sources(state, limit).await?;
    let mut stats = EmbeddingIndexStats {
        sources_seen: sources.len(),
        ..EmbeddingIndexStats::default()
    };

    // Split every source into chunks and flatten to a single (source, chunk_ord)
    // list so the embedding HTTP batches stay full regardless of how chunks are
    // distributed across sources.
    let chunked: Vec<Vec<String>> = sources
        .iter()
        .map(|source| {
            chunk_text(
                &source.text,
                state.config.embedding_max_chars,
                state.config.embedding_chunk_max,
            )
        })
        .collect();
    let refs: Vec<(usize, i32)> = chunked
        .iter()
        .enumerate()
        .flat_map(|(source_idx, chunks)| (0..chunks.len()).map(move |ord| (source_idx, ord as i32)))
        .collect();
    let texts: Vec<&str> = refs
        .iter()
        .map(|&(source_idx, ord)| chunked[source_idx][ord as usize].as_str())
        .collect();

    // Embed the flattened chunks; `embed_batch` re-splits to the server's HTTP
    // batch limit internally.
    let mut vectors = Vec::with_capacity(texts.len());
    for batch in texts.chunks(state.config.embedding_batch_size) {
        let owned: Vec<String> = batch.iter().map(|t| t.to_string()).collect();
        vectors.extend(embedder.embed_batch(&owned).await?);
    }

    // Write each chunk, tracking per-source failures so a bad chunk fails its
    // whole source rather than leaving a half-indexed source marked done.
    let mut failed = vec![false; sources.len()];
    for (&(source_idx, chunk_ord), embedding) in refs.iter().zip(vectors.iter()) {
        if failed[source_idx] {
            continue;
        }
        let source = &sources[source_idx];
        if embedding.len() != state.config.embedding_dimensions as usize {
            let err = format!(
                "embedding dimensions mismatch: expected {}, got {}",
                state.config.embedding_dimensions,
                embedding.len()
            );
            mark_source_failed(state, &source.source_key, &err).await?;
            failed[source_idx] = true;
            stats.failed += 1;
            continue;
        }
        upsert_embedding(state, source, chunk_ord, embedding, vector.column_exists).await?;
        stats.embedded += 1;
    }

    // Finalize each source: drop any stale chunks left over from a longer prior
    // version, then mark it indexed.
    for (source_idx, source) in sources.iter().enumerate() {
        if failed[source_idx] {
            continue;
        }
        let chunk_count = chunked[source_idx].len();
        if chunk_count == 0 {
            // No embeddable text (blank after normalization): clear any chunks
            // and mark indexed so it does not stay pending forever.
            stats.skipped += 1;
        }
        delete_chunks_at_or_above(state, &source.source_key, chunk_count as i32).await?;
        mark_source_indexed(state, source).await?;
    }
    Ok(stats)
}

async fn load_index_status_inner(
    state: &RetrievalState,
    vector: VectorCapabilities,
) -> Result<IndexStatusResponse, RetrievalError> {
    let row = sqlx::query(
        "SELECT \
            COUNT(*) FILTER (WHERE index_status = 'pending' AND deleted_at IS NULL) AS pending_sources, \
            COUNT(*) FILTER (WHERE index_status = 'indexed' AND deleted_at IS NULL) AS indexed_sources, \
            COUNT(*) FILTER (WHERE index_status = 'failed' AND deleted_at IS NULL) AS failed_sources, \
            COUNT(*) FILTER (WHERE deleted_at IS NOT NULL OR index_status = 'deleted') AS deleted_sources, \
            MAX(indexed_at) FILTER (WHERE index_status = 'indexed' AND deleted_at IS NULL) AS latest_indexed_at \
           FROM retrieval_embedding_sources",
    )
    .fetch_one(&state.pool)
    .await?;
    let embedding_count = sqlx::query_scalar(
        "SELECT COUNT(*) \
           FROM retrieval_embeddings \
          WHERE embedding_model = $1 \
            AND embedding_dimensions = $2",
    )
    .bind(&state.config.embedding_model)
    .bind(state.config.embedding_dimensions)
    .fetch_one(&state.pool)
    .await?;
    let backfill = sqlx::query(
        "SELECT \
            COUNT(*) FILTER (WHERE status = 'running') AS running_backfills, \
            COUNT(*) FILTER (WHERE status = 'failed') AS failed_backfills, \
            MAX(generation) AS latest_backfill_generation, \
            COALESCE(SUM(rows_seen), 0)::BIGINT AS backfill_rows_seen, \
            COALESCE(SUM(rows_marked_pending), 0)::BIGINT AS backfill_rows_marked_pending \
           FROM retrieval_embedding_backfills",
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(IndexStatusResponse {
        pending_sources: row.try_get("pending_sources").unwrap_or(0),
        indexed_sources: row.try_get("indexed_sources").unwrap_or(0),
        failed_sources: row.try_get("failed_sources").unwrap_or(0),
        deleted_sources: row.try_get("deleted_sources").unwrap_or(0),
        running_backfills: backfill.try_get("running_backfills").unwrap_or(0),
        failed_backfills: backfill.try_get("failed_backfills").unwrap_or(0),
        latest_backfill_generation: backfill
            .try_get("latest_backfill_generation")
            .ok()
            .flatten(),
        backfill_rows_seen: backfill.try_get("backfill_rows_seen").unwrap_or(0),
        backfill_rows_marked_pending: backfill
            .try_get("backfill_rows_marked_pending")
            .unwrap_or(0),
        embedding_count,
        latest_indexed_at: row.try_get("latest_indexed_at").ok().flatten(),
        vector,
        embedding_model: state.config.embedding_model.clone(),
        embedding_dimensions: state.config.embedding_dimensions,
    })
}

#[derive(Debug, Clone)]
struct EmbeddingSource {
    source_family: SourceFamily,
    source_kind: SourceKind,
    source_key: String,
    session_uuid: Uuid,
    byte_offset: Option<i64>,
    block_ord: Option<i32>,
    turn_id: Option<i64>,
    operation_ord: Option<i32>,
    repo_name: Option<String>,
    content_hash: String,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceFamily {
    EventBlock,
    OperationCall,
    OperationResult,
}

impl SourceFamily {
    fn as_str(self) -> &'static str {
        match self {
            Self::EventBlock => "event_block",
            Self::OperationCall => "operation_call",
            Self::OperationResult => "operation_result",
        }
    }

    fn from_db(raw: &str) -> Result<Self, RetrievalError> {
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

struct PendingSourceRow {
    source_family: SourceFamily,
    source_key: String,
    session_uuid: Uuid,
    byte_offset: Option<i64>,
    block_ord: Option<i32>,
    turn_id: Option<i64>,
    operation_ord: Option<i32>,
}

async fn next_backfill_generation(state: &RetrievalState) -> Result<i64, RetrievalError> {
    let generation =
        sqlx::query_scalar("SELECT nextval('retrieval_embedding_backfill_generation_seq')::BIGINT")
            .fetch_one(&state.pool)
            .await?;
    Ok(generation)
}

async fn start_backfill_runs(
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

async fn advance_backfill_runs(state: &RetrievalState, limit: i64) -> Result<(), RetrievalError> {
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

fn source_from_row(
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

async fn load_pending_embedding_sources(
    state: &RetrievalState,
    limit: i64,
) -> Result<Vec<EmbeddingSource>, RetrievalError> {
    let pending = load_pending_source_rows(state, limit).await?;
    let mut sources = Vec::new();
    for pending_source in pending {
        match load_current_source(state, &pending_source).await? {
            Some(source) => sources.push(source),
            None => mark_source_deleted(state, &pending_source.source_key).await?,
        }
    }
    Ok(sources)
}

async fn load_pending_source_rows(
    state: &RetrievalState,
    limit: i64,
) -> Result<Vec<PendingSourceRow>, RetrievalError> {
    let rows = sqlx::query(
        "SELECT source_family, source_key, session_uuid, byte_offset, block_ord, turn_id, operation_ord \
           FROM retrieval_embedding_sources \
          WHERE index_status = 'pending' \
            AND deleted_at IS NULL \
          ORDER BY dirty_at ASC, id ASC \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let family: String = row.try_get("source_family")?;
            Ok(PendingSourceRow {
                source_family: SourceFamily::from_db(&family)?,
                source_key: row.try_get("source_key")?,
                session_uuid: row.try_get("session_uuid")?,
                byte_offset: row.try_get("byte_offset")?,
                block_ord: row.try_get("block_ord")?,
                turn_id: row.try_get("turn_id")?,
                operation_ord: row.try_get("operation_ord")?,
            })
        })
        .collect()
}

async fn load_current_source(
    state: &RetrievalState,
    pending: &PendingSourceRow,
) -> Result<Option<EmbeddingSource>, RetrievalError> {
    match pending.source_family {
        SourceFamily::EventBlock => load_current_event_source(state, pending).await,
        SourceFamily::OperationCall => load_current_operation_source(state, pending, false).await,
        SourceFamily::OperationResult => load_current_operation_source(state, pending, true).await,
    }
}

async fn load_current_event_source(
    state: &RetrievalState,
    pending: &PendingSourceRow,
) -> Result<Option<EmbeddingSource>, RetrievalError> {
    let Some(byte_offset) = pending.byte_offset else {
        return Ok(None);
    };
    let Some(block_ord) = pending.block_ord else {
        return Ok(None);
    };
    let row = sqlx::query(
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
          WHERE e.session_uuid = $1 \
            AND e.byte_offset = $2 \
            AND b.ord = $3 \
            AND b.text IS NOT NULL \
            AND length(trim(b.text)) > 0 \
            AND ( \
                (e.speaker IN ('assistant', 'user', 'summary') AND b.kind = 'text') \
                OR (b.kind = 'tool_result' AND COALESCE(b.is_error, FALSE)) \
            )",
    )
    .bind(pending.session_uuid)
    .bind(byte_offset)
    .bind(block_ord)
    .fetch_optional(&state.pool)
    .await?;
    row.map(|row| source_from_row(row, SourceFamily::EventBlock))
        .transpose()
        .map(Option::flatten)
}

async fn load_current_operation_source(
    state: &RetrievalState,
    pending: &PendingSourceRow,
    result_source: bool,
) -> Result<Option<EmbeddingSource>, RetrievalError> {
    let Some(turn_id) = pending.turn_id else {
        return Ok(None);
    };
    let Some(operation_ord) = pending.operation_ord else {
        return Ok(None);
    };
    let sql = if result_source {
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
          WHERE o.session_uuid = $1 \
            AND o.turn_id = $2 \
            AND o.operation_ord = $3 \
            AND (o.result_content IS NOT NULL OR o.result_payload IS NOT NULL OR o.result_is_error OR o.is_error) \
            AND length(trim(concat_ws(' ', o.name, o.result_content, o.result_payload::TEXT))) > 0 \
            AND (o.result_is_error OR o.is_error OR o.name = 'agent')"
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
          WHERE o.session_uuid = $1 \
            AND o.turn_id = $2 \
            AND o.operation_ord = $3 \
            AND o.input IS NOT NULL \
            AND length(trim(concat_ws(' ', o.name, o.raw_name, o.operation_type, o.operation_category, o.input::TEXT))) > 0"
    };
    let row = sqlx::query(sql)
        .bind(pending.session_uuid)
        .bind(turn_id)
        .bind(operation_ord)
        .fetch_optional(&state.pool)
        .await?;
    let family = if result_source {
        SourceFamily::OperationResult
    } else {
        SourceFamily::OperationCall
    };
    row.map(|row| source_from_row(row, family))
        .transpose()
        .map(Option::flatten)
}

async fn mark_source_deleted(
    state: &RetrievalState,
    source_key: &str,
) -> Result<(), RetrievalError> {
    sqlx::query("DELETE FROM retrieval_embeddings WHERE source_key = $1")
        .bind(source_key)
        .execute(&state.pool)
        .await?;
    sqlx::query(
        "UPDATE retrieval_embedding_sources \
            SET index_status = 'deleted', deleted_at = NOW(), updated_at = NOW() \
          WHERE source_key = $1",
    )
    .bind(source_key)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn upsert_embedding(
    state: &RetrievalState,
    source: &EmbeddingSource,
    chunk_ord: i32,
    embedding: &[f32],
    vector_column_exists: bool,
) -> Result<(), RetrievalError> {
    if vector_column_exists {
        sqlx::query(
            "INSERT INTO retrieval_embeddings \
                (source_kind, source_key, session_uuid, byte_offset, block_ord, turn_id, operation_ord, repo_name, \
                 content_hash, embedding_model, embedding_dimensions, embedding, embedding_vector, chunk_ord, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13::vector, $14, NOW()) \
             ON CONFLICT (embedding_model, source_key, chunk_ord) DO UPDATE SET \
                 content_hash = EXCLUDED.content_hash, \
                 embedding_dimensions = EXCLUDED.embedding_dimensions, \
                 embedding = EXCLUDED.embedding, \
                 embedding_vector = EXCLUDED.embedding_vector, \
                 operation_ord = EXCLUDED.operation_ord, \
                 updated_at = NOW()",
        )
        .bind(source.source_kind.as_str())
        .bind(&source.source_key)
        .bind(source.session_uuid)
        .bind(source.byte_offset)
        .bind(source.block_ord)
        .bind(source.turn_id)
        .bind(source.operation_ord)
        .bind(source.repo_name.as_deref())
        .bind(&source.content_hash)
        .bind(&state.config.embedding_model)
        .bind(state.config.embedding_dimensions)
        .bind(embedding)
        .bind(vector_literal(embedding))
        .bind(chunk_ord)
        .execute(&state.pool)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO retrieval_embeddings \
                (source_kind, source_key, session_uuid, byte_offset, block_ord, turn_id, operation_ord, repo_name, \
                 content_hash, embedding_model, embedding_dimensions, embedding, chunk_ord, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, NOW()) \
             ON CONFLICT (embedding_model, source_key, chunk_ord) DO UPDATE SET \
                 content_hash = EXCLUDED.content_hash, \
                 embedding_dimensions = EXCLUDED.embedding_dimensions, \
                 embedding = EXCLUDED.embedding, \
                 operation_ord = EXCLUDED.operation_ord, \
                 updated_at = NOW()",
        )
        .bind(source.source_kind.as_str())
        .bind(&source.source_key)
        .bind(source.session_uuid)
        .bind(source.byte_offset)
        .bind(source.block_ord)
        .bind(source.turn_id)
        .bind(source.operation_ord)
        .bind(source.repo_name.as_deref())
        .bind(&source.content_hash)
        .bind(&state.config.embedding_model)
        .bind(state.config.embedding_dimensions)
        .bind(embedding)
        .bind(chunk_ord)
        .execute(&state.pool)
        .await?;
    }
    Ok(())
}

/// Remove embedding rows for a source at or above `keep_below`. Used after a
/// re-index to drop chunks left over when a source's text got shorter (fewer
/// chunks than before).
async fn delete_chunks_at_or_above(
    state: &RetrievalState,
    source_key: &str,
    keep_below: i32,
) -> Result<(), RetrievalError> {
    sqlx::query(
        "DELETE FROM retrieval_embeddings \
          WHERE embedding_model = $1 AND source_key = $2 AND chunk_ord >= $3",
    )
    .bind(&state.config.embedding_model)
    .bind(source_key)
    .bind(keep_below)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn mark_source_indexed(
    state: &RetrievalState,
    source: &EmbeddingSource,
) -> Result<(), RetrievalError> {
    sqlx::query(
        "UPDATE retrieval_embedding_sources \
            SET source_kind = $2, \
                session_uuid = $3, \
                byte_offset = $4, \
                block_ord = $5, \
                turn_id = $6, \
                operation_ord = $7, \
                repo_name = $8, \
                content_hash = $9, \
                index_status = 'indexed', \
                index_error = NULL, \
                indexed_at = NOW(), \
                last_seen_at = NOW(), \
                deleted_at = NULL, \
                updated_at = NOW() \
          WHERE source_key = $1",
    )
    .bind(&source.source_key)
    .bind(source.source_kind.as_str())
    .bind(source.session_uuid)
    .bind(source.byte_offset)
    .bind(source.block_ord)
    .bind(source.turn_id)
    .bind(source.operation_ord)
    .bind(source.repo_name.as_deref())
    .bind(&source.content_hash)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn mark_source_failed(
    state: &RetrievalState,
    source_key: &str,
    error: &str,
) -> Result<(), RetrievalError> {
    sqlx::query(
        "UPDATE retrieval_embedding_sources \
            SET index_status = 'failed', \
                index_error = $2, \
                updated_at = NOW() \
          WHERE source_key = $1",
    )
    .bind(source_key)
    .bind(error)
    .execute(&state.pool)
    .await?;
    Ok(())
}

fn hash_text(text: &str) -> String {
    let hash = digest::digest(&digest::SHA256, text.as_bytes());
    hash.as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Split a source's text into embeddable chunks. Whitespace is collapsed, then
/// the text is cut into `max_chars`-sized chunks, capped at `max_chunks` (the
/// tail beyond the cap is dropped). Short sources (the common case, including the
/// capped tool_call/tool_error inputs) yield a single chunk. Returns an empty
/// vec for blank text.
fn chunk_text(text: &str, max_chars: usize, max_chunks: usize) -> Vec<String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = normalized.chars().collect();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() && chunks.len() < max_chunks {
        let end = (start + max_chars).min(chars.len());
        chunks.push(chars[start..end].iter().collect());
        start = end;
    }
    chunks
}

#[derive(Clone)]
pub(super) struct EmbeddingClient {
    pub(super) http: reqwest::Client,
    pub(super) service_url: String,
    pub(super) model: String,
    pub(super) dimensions: i32,
}

/// Embedding servers cap the number of inputs per request — text-embeddings-
/// inference defaults to `--max-client-batch-size 32` and returns HTTP 422
/// ("batch size N > maximum allowed batch size 32") above it. The indexer's
/// logical `embedding_batch_size` is a DB-processing granularity that may be
/// larger, so each embedding call is split into HTTP sub-batches no larger than
/// this. 32 is TEI's default and a safe lower bound for other servers.
const EMBEDDING_HTTP_MAX_BATCH: usize = 32;

impl EmbeddingClient {
    pub(super) async fn embed_one(&self, text: &str) -> Result<Vec<f32>, RetrievalError> {
        let mut embeddings = self.embed_batch(&[text.to_string()]).await?;
        embeddings
            .pop()
            .ok_or_else(|| RetrievalError::unavailable("embedding response was empty"))
    }

    pub(super) async fn embed_batch(
        &self,
        input: &[String],
    ) -> Result<Vec<Vec<f32>>, RetrievalError> {
        let mut out = Vec::with_capacity(input.len());
        for chunk in input.chunks(EMBEDDING_HTTP_MAX_BATCH) {
            out.extend(self.embed_http_batch(chunk).await?);
        }
        Ok(out)
    }

    /// One embedding request for an input batch already within the server's
    /// per-request limit (see [`EMBEDDING_HTTP_MAX_BATCH`]). Retries transient
    /// failures up to three times.
    async fn embed_http_batch(&self, input: &[String]) -> Result<Vec<Vec<f32>>, RetrievalError> {
        if input.is_empty() {
            return Ok(Vec::new());
        }
        let mut last_error = None;
        let endpoint = format!("{}/v1/embeddings", self.service_url.trim_end_matches('/'));
        for attempt in 1..=3 {
            let result = self
                .http
                .post(&endpoint)
                .json(&EmbeddingRequest {
                    model: self.model.as_str(),
                    input,
                    encoding_format: "float",
                    dimensions: Some(self.dimensions),
                })
                .send()
                .await
                .map_err(|err| {
                    RetrievalError::unavailable(format!("embedding request failed: {err}"))
                })
                .and_then(|response| {
                    response.error_for_status().map_err(|err| {
                        RetrievalError::unavailable(format!("embedding request failed: {err}"))
                    })
                });
            match result {
                Ok(response) => {
                    let response = response.json::<EmbeddingResponse>().await.map_err(|err| {
                        RetrievalError::unavailable(format!("invalid embedding response: {err}"))
                    })?;
                    return validate_embedding_response(response, input.len());
                }
                Err(err) => {
                    last_error = Some(err);
                    if attempt < 3 {
                        tokio::time::sleep(Duration::from_millis(100 * attempt)).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| RetrievalError::unavailable("embedding request failed")))
    }
}

fn validate_embedding_response(
    response: EmbeddingResponse,
    input_len: usize,
) -> Result<Vec<Vec<f32>>, RetrievalError> {
    let mut data = response.data;
    data.sort_by_key(|item| item.index);
    if data.len() != input_len {
        return Err(RetrievalError::unavailable(format!(
            "embedding response length mismatch: expected {}, got {}",
            input_len,
            data.len()
        )));
    }
    Ok(data.into_iter().map(|item| item.embedding).collect())
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
    encoding_format: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<i32>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
    #[allow(dead_code)]
    model: Option<String>,
    #[allow(dead_code)]
    usage: Option<Value>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    index: i32,
    embedding: Vec<f32>,
}
