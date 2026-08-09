use super::*;

mod backfill;
mod client;

use backfill::{
    advance_backfill_runs, next_backfill_generation, source_from_row, start_backfill_runs,
    EmbeddingSource, PendingSourceRow, SourceFamily,
};
pub(super) use client::EmbeddingClient;

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
    let vector = *state.vector_capabilities.read().await;
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
    let vector = *state.vector_capabilities.read().await;
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
    if sources.is_empty() {
        return Ok(stats);
    }

    // Split every source into chunks and flatten the text so the embedding HTTP
    // batches stay full regardless of how chunks are distributed across sources.
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
    let texts: Vec<&str> = chunked
        .iter()
        .flat_map(|chunks| chunks.iter().map(String::as_str))
        .collect();

    // Embed the flattened chunks; `embed_batch` re-splits to the server's HTTP
    // batch limit internally.
    let mut vectors = Vec::with_capacity(texts.len());
    for batch in texts.chunks(state.config.embedding_batch_size) {
        let owned: Vec<String> = batch.iter().map(|t| t.to_string()).collect();
        vectors.extend(embedder.embed_batch(&owned).await?);
    }
    if vectors.len() != texts.len() {
        return Err(RetrievalError::internal(anyhow!(
            "embedding service returned {} vectors for {} chunks",
            vectors.len(),
            texts.len()
        )));
    }

    // Persist the bounded source batch atomically. Each source may require
    // several chunk upserts, a stale-chunk delete, and a status update; keeping
    // them in one transaction removes the per-statement commit/WAL flushes and
    // prevents a partially refreshed source from becoming visible.
    let mut tx = state.pool.begin().await?;
    let mut vector_offset = 0;
    for (source, chunks) in sources.iter().zip(&chunked) {
        let source_vectors = &vectors[vector_offset..vector_offset + chunks.len()];
        vector_offset += chunks.len();
        if let Some(embedding) = source_vectors
            .iter()
            .find(|embedding| embedding.len() != state.config.embedding_dimensions as usize)
        {
            let err = format!(
                "embedding dimensions mismatch: expected {}, got {}",
                state.config.embedding_dimensions,
                embedding.len()
            );
            mark_source_failed(&mut tx, &source.source_key, &err).await?;
            stats.failed += 1;
            continue;
        }

        for (chunk_ord, embedding) in source_vectors.iter().enumerate() {
            upsert_embedding(
                &mut tx,
                state,
                source,
                chunk_ord as i32,
                embedding,
                vector.column_exists,
            )
            .await?;
            stats.embedded += 1;
        }
        let chunk_count = chunks.len();
        if chunk_count == 0 {
            // No embeddable text (blank after normalization): clear any chunks
            // and mark indexed so it does not stay pending forever.
            stats.skipped += 1;
        }
        delete_chunks_at_or_above(&mut tx, state, &source.source_key, chunk_count as i32).await?;
        mark_source_indexed(&mut tx, source).await?;
    }
    tx.commit().await?;
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

async fn load_pending_embedding_sources(
    state: &RetrievalState,
    limit: i64,
) -> Result<Vec<EmbeddingSource>, RetrievalError> {
    let pending = load_pending_source_rows(state, limit).await?;
    let mut sources = Vec::new();
    let mut deleted_source_keys = Vec::new();
    for pending_source in pending {
        match load_current_source(state, &pending_source).await? {
            Some(source) => sources.push(source),
            None => deleted_source_keys.push(pending_source.source_key),
        }
    }
    if !deleted_source_keys.is_empty() {
        mark_sources_deleted(state, &deleted_source_keys).await?;
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
            COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/sulion/repos/%' THEN split_part(substr(asm.cwd, length('/home/sulion/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/sulion/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/sulion/workspaces/') + 1), '/', 1) ELSE NULL END) AS repo_name, \
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
            COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/sulion/repos/%' THEN split_part(substr(asm.cwd, length('/home/sulion/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/sulion/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/sulion/workspaces/') + 1), '/', 1) ELSE NULL END) AS repo_name, \
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
            COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/sulion/repos/%' THEN split_part(substr(asm.cwd, length('/home/sulion/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/sulion/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/sulion/workspaces/') + 1), '/', 1) ELSE NULL END) AS repo_name, \
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

async fn mark_sources_deleted(
    state: &RetrievalState,
    source_keys: &[String],
) -> Result<(), RetrievalError> {
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM retrieval_embeddings WHERE source_key = ANY($1)")
        .bind(source_keys)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE retrieval_embedding_sources \
            SET index_status = 'deleted', deleted_at = NOW(), updated_at = NOW() \
          WHERE source_key = ANY($1)",
    )
    .bind(source_keys)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn upsert_embedding(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
        .execute(&mut **tx)
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
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Remove embedding rows for a source at or above `keep_below`. Used after a
/// re-index to drop chunks left over when a source's text got shorter (fewer
/// chunks than before).
async fn delete_chunks_at_or_above(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn mark_source_indexed(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn mark_source_failed(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
    .execute(&mut **tx)
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
