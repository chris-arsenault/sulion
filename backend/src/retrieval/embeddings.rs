use super::*;

#[derive(Debug, Deserialize)]
pub(super) struct ReindexRequest {
    pub(super) repo: Option<String>,
    pub(super) agent_session_uuid: Option<Uuid>,
    pub(super) limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub(super) struct ReindexResponse {
    embedded: usize,
    skipped: usize,
    vector: VectorCapabilities,
    embedding_model: String,
    embedding_dimensions: i32,
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
    let embedder = state.embedding_client();
    let vector = state.refresh_vector_capabilities().await?;
    let limit = request
        .limit
        .unwrap_or(DEFAULT_REINDEX_LIMIT)
        .clamp(1, MAX_REINDEX_LIMIT);
    let sources = load_embedding_sources(
        &state.pool,
        request.repo.as_deref(),
        request.agent_session_uuid,
        &state.config.embedding_model,
        state.config.embedding_dimensions,
        limit,
    )
    .await?;
    let mut embedded = 0_usize;
    let mut skipped = 0_usize;
    for chunk in sources.chunks(state.config.embedding_batch_size) {
        let texts = chunk
            .iter()
            .map(|source| embedding_text(&source.text, state.config.embedding_max_chars))
            .collect::<Vec<_>>();
        let vectors = embedder.embed_batch(&texts).await?;
        for (source, embedding) in chunk.iter().zip(vectors) {
            if embedding.len() != state.config.embedding_dimensions as usize {
                return Err(RetrievalError::unavailable(format!(
                    "embedding dimensions mismatch: expected {}, got {}",
                    state.config.embedding_dimensions,
                    embedding.len()
                )));
            }
            if source.text.trim().is_empty() {
                skipped += 1;
                continue;
            }
            upsert_embedding(state, source, &embedding, vector.column_exists).await?;
            embedded += 1;
        }
    }
    Ok(ReindexResponse {
        embedded,
        skipped,
        vector,
        embedding_model: state.config.embedding_model.clone(),
        embedding_dimensions: state.config.embedding_dimensions,
    })
}

#[derive(Debug, Clone)]
struct EmbeddingSource {
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

async fn load_embedding_sources(
    pool: &Pool,
    repo: Option<&str>,
    session_uuid: Option<Uuid>,
    embedding_model: &str,
    embedding_dimensions: i32,
    limit: i64,
) -> Result<Vec<EmbeddingSource>, RetrievalError> {
    let rows = sqlx::query(
        "WITH source_rows AS ( \
             SELECT \
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
                b.text \
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
               AND ($1::UUID IS NULL OR e.session_uuid = $1) \
               AND ($2::TEXT IS NULL OR COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) = $2) \
             UNION ALL \
             SELECT \
                'tool_call' AS source_kind, \
                ('operation:' || o.session_uuid::TEXT || ':' || o.turn_id::TEXT || ':' || o.operation_ord::TEXT || ':call') AS source_key, \
                o.session_uuid, NULL::BIGINT AS byte_offset, NULL::INT AS block_ord, o.turn_id, o.operation_ord, \
                COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) AS repo_name, \
                concat_ws(' ', o.name, o.raw_name, o.operation_type, o.operation_category, o.input::TEXT) AS text \
             FROM timeline_operations o \
             JOIN claude_sessions cs ON cs.session_uuid = o.session_uuid \
             LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
             LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
             WHERE o.input IS NOT NULL \
               AND ($1::UUID IS NULL OR o.session_uuid = $1) \
               AND ($2::TEXT IS NULL OR COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) = $2) \
             UNION ALL \
             SELECT \
                CASE WHEN o.result_is_error OR o.is_error THEN 'tool_error' ELSE 'tool_result' END AS source_kind, \
                ('operation:' || o.session_uuid::TEXT || ':' || o.turn_id::TEXT || ':' || o.operation_ord::TEXT || ':result') AS source_key, \
                o.session_uuid, NULL::BIGINT AS byte_offset, NULL::INT AS block_ord, o.turn_id, o.operation_ord, \
                COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) AS repo_name, \
                concat_ws(' ', o.name, o.result_content, o.result_payload::TEXT) AS text \
             FROM timeline_operations o \
             JOIN claude_sessions cs ON cs.session_uuid = o.session_uuid \
             LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
             LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
             WHERE (o.result_content IS NOT NULL OR o.result_payload IS NOT NULL OR o.result_is_error OR o.is_error) \
               AND ($1::UUID IS NULL OR o.session_uuid = $1) \
               AND ($2::TEXT IS NULL OR COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) = $2) \
        ) \
        SELECT s.*, re.content_hash AS existing_hash, re.embedding_dimensions AS existing_dimensions \
          FROM source_rows s \
          LEFT JOIN retrieval_embeddings re \
            ON re.embedding_model = $3 \
           AND re.source_key = s.source_key \
         WHERE s.source_kind IS NOT NULL \
           AND length(trim(s.text)) > 0 \
         ORDER BY s.session_uuid ASC, s.byte_offset ASC, s.block_ord ASC \
         LIMIT $4",
    )
    .bind(session_uuid)
    .bind(repo)
    .bind(embedding_model)
    .bind(limit.saturating_mul(4).max(limit))
    .fetch_all(pool)
    .await?;

    let mut sources = Vec::new();
    for row in rows {
        let kind: String = match row.try_get("source_kind") {
            Ok(kind) => kind,
            Err(_) => continue,
        };
        let text: String = match row.try_get("text") {
            Ok(text) => text,
            Err(_) => continue,
        };
        let source_kind = match kind.as_str() {
            "assistant_text" => SourceKind::AssistantText,
            "user_prompt" => SourceKind::UserPrompt,
            "summary" => SourceKind::Summary,
            "tool_call" => SourceKind::ToolCall,
            "tool_result" => SourceKind::ToolResult,
            "tool_error" => SourceKind::ToolError,
            _ => continue,
        };
        let content_hash = hash_text(&text);
        let existing_hash: Option<String> = row.try_get("existing_hash").ok().flatten();
        let existing_dimensions: Option<i32> = row.try_get("existing_dimensions").ok().flatten();
        if existing_hash.as_deref() == Some(content_hash.as_str())
            && existing_dimensions == Some(embedding_dimensions)
        {
            continue;
        }
        sources.push(EmbeddingSource {
            source_kind,
            source_key: row.try_get("source_key")?,
            session_uuid: row.try_get("session_uuid")?,
            byte_offset: row.try_get("byte_offset").ok(),
            block_ord: row.try_get("block_ord").ok(),
            turn_id: row.try_get("turn_id").ok().flatten(),
            operation_ord: row.try_get("operation_ord").ok().flatten(),
            repo_name: row.try_get("repo_name").ok().flatten(),
            content_hash,
            text,
        });
        if sources.len() >= limit as usize {
            break;
        }
    }
    Ok(sources)
}

async fn upsert_embedding(
    state: &RetrievalState,
    source: &EmbeddingSource,
    embedding: &[f32],
    vector_column_exists: bool,
) -> Result<(), RetrievalError> {
    if vector_column_exists {
        sqlx::query(
            "INSERT INTO retrieval_embeddings \
                (source_kind, source_key, session_uuid, byte_offset, block_ord, turn_id, operation_ord, repo_name, \
                 content_hash, embedding_model, embedding_dimensions, embedding, embedding_vector, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13::vector, NOW()) \
             ON CONFLICT (embedding_model, source_key) DO UPDATE SET \
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
        .execute(&state.pool)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO retrieval_embeddings \
                (source_kind, source_key, session_uuid, byte_offset, block_ord, turn_id, operation_ord, repo_name, \
                 content_hash, embedding_model, embedding_dimensions, embedding, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, NOW()) \
             ON CONFLICT (embedding_model, source_key) DO UPDATE SET \
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
        .execute(&state.pool)
        .await?;
    }
    Ok(())
}

fn hash_text(text: &str) -> String {
    let hash = digest::digest(&digest::SHA256, text.as_bytes());
    hash.as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn embedding_text(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    normalized.chars().take(max_chars).collect()
}

#[derive(Clone)]
pub(super) struct EmbeddingClient {
    pub(super) http: reqwest::Client,
    pub(super) service_url: String,
    pub(super) model: String,
    pub(super) dimensions: i32,
}

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
