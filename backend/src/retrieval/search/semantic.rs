use super::*;

pub(super) async fn semantic_search(
    state: &RetrievalState,
    query_embedding: &[f32],
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, RetrievalError> {
    let capabilities = *state.vector_capabilities.read().await;
    if capabilities.column_exists {
        semantic_search_pgvector(state, query_embedding, filters).await
    } else {
        semantic_search_exact(
            &state.pool,
            query_embedding,
            filters,
            state.config.semantic_min_score,
        )
        .await
    }
}

async fn semantic_search_pgvector(
    state: &RetrievalState,
    query_embedding: &[f32],
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, RetrievalError> {
    let low_value_tool_names = low_value_tool_names();
    let include = included_source_kinds(filters);
    let vector = vector_literal(query_embedding);
    let rows = sqlx::query(
        "WITH cand AS ( \
             SELECT re.source_key, re.source_kind, re.session_uuid, re.byte_offset, re.block_ord, re.turn_id, re.operation_ord, \
                    re.repo_name, (re.embedding_vector <=> $1::vector) AS dist \
               FROM retrieval_embeddings re \
               LEFT JOIN timeline_operations o_filter ON o_filter.session_uuid = re.session_uuid AND o_filter.turn_id = re.turn_id AND o_filter.operation_ord = re.operation_ord \
              WHERE re.embedding_model = $2 \
                AND re.embedding_dimensions = $3 \
                AND re.embedding_vector IS NOT NULL \
                AND re.source_kind = ANY($4) \
                AND ($5::UUID IS NULL OR re.session_uuid = $5) \
                AND ($6::TEXT IS NULL OR re.repo_name = $6) \
                AND (1.0 - (re.embedding_vector <=> $1::vector)) >= $17 \
                AND ($18::BOOLEAN OR NOT ( \
                     re.source_kind IN ('tool_call', 'tool_result', 'tool_error') \
                     AND ( \
                         lower(COALESCE(o_filter.name, '')) = ANY($19::TEXT[]) \
                         OR lower(COALESCE(o_filter.raw_name, '')) = ANY($19::TEXT[]) \
                         OR lower(COALESCE(o_filter.operation_type, '')) = ANY($19::TEXT[]) \
                     ) \
                )) \
              ORDER BY re.embedding_vector <=> $1::vector \
              LIMIT $7 \
        ), \
        ranked AS ( \
             SELECT DISTINCT ON (source_key) source_kind, session_uuid, byte_offset, block_ord, turn_id, operation_ord, \
                    repo_name, (1.0 - dist)::REAL AS semantic_score \
               FROM cand \
              ORDER BY source_key, dist \
        ) \
        SELECT r.source_kind, r.session_uuid, r.byte_offset, r.block_ord, r.turn_id, r.operation_ord, \
               COALESCE(e.timestamp, tt.end_timestamp) AS timestamp, cs.agent, cs.pty_session_id, ps.repo AS pty_repo, asm.cwd, asm.model, \
               CASE \
                 WHEN r.source_kind = 'tool_call' THEN concat_ws(' ', o.name, o.raw_name, o.operation_type, o.operation_category, o.input::TEXT) \
                 WHEN r.source_kind IN ('tool_result', 'tool_error') THEN concat_ws(' ', o.name, o.result_content, o.result_payload::TEXT) \
                 ELSE b.text \
               END AS text, \
               o.name, o.raw_name, o.operation_type, o.operation_category, o.input, \
               o.result_content, o.result_payload, o.is_error, o.result_is_error, \
               r.semantic_score, tt.preview AS turn_preview \
          FROM ranked r \
          JOIN claude_sessions cs ON cs.session_uuid = r.session_uuid \
          LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
          LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
          LEFT JOIN events e ON e.session_uuid = r.session_uuid AND e.byte_offset = r.byte_offset \
          LEFT JOIN event_blocks b ON b.session_uuid = r.session_uuid AND b.byte_offset = r.byte_offset AND b.ord = r.block_ord \
          LEFT JOIN timeline_turns tt ON tt.session_uuid = r.session_uuid AND tt.turn_id = r.turn_id \
          LEFT JOIN timeline_operations o ON o.session_uuid = r.session_uuid AND o.turn_id = r.turn_id AND o.operation_ord = r.operation_ord \
         WHERE ($8::TEXT IS NULL OR cs.agent = $8) \
           AND ($9::TEXT IS NULL OR asm.model = $9) \
           AND ($10::TIMESTAMPTZ IS NULL OR COALESCE(e.timestamp, tt.end_timestamp) >= $10) \
           AND ($11::TIMESTAMPTZ IS NULL OR COALESCE(e.timestamp, tt.end_timestamp) <= $11) \
           AND ($12::BOOLEAN = FALSE OR ( \
                r.source_kind IN ('tool_call', 'tool_result', 'tool_error') \
                AND (COALESCE(o.is_error, FALSE) OR COALESCE(o.result_is_error, FALSE)) \
           ) OR ( \
                r.source_kind NOT IN ('tool_call', 'tool_result', 'tool_error') \
                AND EXISTS ( \
                    SELECT 1 FROM timeline_operations ofilter \
                     WHERE ofilter.session_uuid = r.session_uuid \
                       AND ofilter.turn_id = r.turn_id \
                       AND (ofilter.is_error OR ofilter.result_is_error) \
                ) \
           )) \
           AND ($13::TEXT IS NULL OR EXISTS ( \
                SELECT 1 FROM timeline_file_touches ft \
                 WHERE ft.session_uuid = r.session_uuid \
                   AND ft.turn_id = r.turn_id \
                   AND ft.repo_rel_path = $13 \
           )) \
           AND ($14::TEXT IS NULL OR ( \
                r.source_kind IN ('tool_call', 'tool_result', 'tool_error') AND o.operation_category = $14 \
           ) OR ( \
                r.source_kind NOT IN ('tool_call', 'tool_result', 'tool_error') \
                AND EXISTS ( \
                    SELECT 1 FROM timeline_operations ofilter \
                     WHERE ofilter.session_uuid = r.session_uuid \
                       AND ofilter.turn_id = r.turn_id \
                       AND ofilter.operation_category = $14 \
                ) \
           )) \
           AND ($15::TEXT IS NULL OR ( \
                r.source_kind IN ('tool_call', 'tool_result', 'tool_error') AND o.name = $15 \
           ) OR ( \
                r.source_kind NOT IN ('tool_call', 'tool_result', 'tool_error') \
                AND EXISTS ( \
                    SELECT 1 FROM timeline_operations ofilter \
                     WHERE ofilter.session_uuid = r.session_uuid \
                       AND ofilter.turn_id = r.turn_id \
                       AND ofilter.name = $15 \
                ) \
           )) \
         ORDER BY r.semantic_score DESC \
         LIMIT $16",
    )
    .bind(vector)
    .bind(&state.config.embedding_model)
    .bind(state.config.embedding_dimensions)
    .bind(&include)
    .bind(scoped_session(filters))
    .bind(scoped_repo(filters))
    .bind(filters.limit * 4)
    .bind(filters.agent.as_deref())
    .bind(filters.model.as_deref())
    .bind(filters.since)
    .bind(filters.until)
    .bind(filters.errors_only)
    .bind(filters.file_path.as_deref())
    .bind(filters.tool_category.as_deref())
    .bind(filters.tool_name.as_deref())
    .bind(filters.limit)
    .bind(state.config.semantic_min_score)
    .bind(filters.include_low_value)
    .bind(&low_value_tool_names)
    .fetch_all(&state.pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|row| row_to_any_search_result(row, Some("semantic")))
        .collect())
}

async fn semantic_search_exact(
    pool: &Pool,
    query_embedding: &[f32],
    filters: &SearchFilters,
    min_score: f32,
) -> Result<Vec<SearchResult>, RetrievalError> {
    let low_value_tool_names = low_value_tool_names();
    let include = included_source_kinds(filters);
    let rows = sqlx::query(
        "SELECT re.source_key, re.source_kind, re.session_uuid, re.byte_offset, re.block_ord, re.turn_id, re.operation_ord, \
                re.embedding, COALESCE(e.timestamp, tt.end_timestamp) AS timestamp, cs.agent, cs.pty_session_id, ps.repo AS pty_repo, \
                asm.cwd, asm.model, \
                CASE \
                  WHEN re.source_kind = 'tool_call' THEN concat_ws(' ', o.name, o.raw_name, o.operation_type, o.operation_category, o.input::TEXT) \
                  WHEN re.source_kind IN ('tool_result', 'tool_error') THEN concat_ws(' ', o.name, o.result_content, o.result_payload::TEXT) \
                  ELSE b.text \
                END AS text, \
                o.name, o.raw_name, o.operation_type, o.operation_category, o.input, \
                o.result_content, o.result_payload, o.is_error, o.result_is_error, \
                tt.preview AS turn_preview \
           FROM retrieval_embeddings re \
           JOIN claude_sessions cs ON cs.session_uuid = re.session_uuid \
           LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
           LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
           LEFT JOIN events e ON e.session_uuid = re.session_uuid AND e.byte_offset = re.byte_offset \
           LEFT JOIN event_blocks b ON b.session_uuid = re.session_uuid AND b.byte_offset = re.byte_offset AND b.ord = re.block_ord \
           LEFT JOIN timeline_turns tt ON tt.session_uuid = re.session_uuid AND tt.turn_id = re.turn_id \
           LEFT JOIN timeline_operations o ON o.session_uuid = re.session_uuid AND o.turn_id = re.turn_id AND o.operation_ord = re.operation_ord \
          WHERE re.source_kind = ANY($1) \
            AND ($2::UUID IS NULL OR re.session_uuid = $2) \
            AND ($3::TEXT IS NULL OR re.repo_name = $3) \
            AND ($4::TEXT IS NULL OR cs.agent = $4) \
            AND ($5::TEXT IS NULL OR asm.model = $5) \
            AND ($6::TIMESTAMPTZ IS NULL OR COALESCE(e.timestamp, tt.end_timestamp) >= $6) \
            AND ($7::TIMESTAMPTZ IS NULL OR COALESCE(e.timestamp, tt.end_timestamp) <= $7) \
            AND ($8::BOOLEAN = FALSE OR ( \
                 re.source_kind IN ('tool_call', 'tool_result', 'tool_error') \
                 AND (COALESCE(o.is_error, FALSE) OR COALESCE(o.result_is_error, FALSE)) \
            ) OR ( \
                 re.source_kind NOT IN ('tool_call', 'tool_result', 'tool_error') \
                 AND EXISTS ( \
                     SELECT 1 FROM timeline_operations ofilter \
                      WHERE ofilter.session_uuid = re.session_uuid \
                        AND ofilter.turn_id = re.turn_id \
                        AND (ofilter.is_error OR ofilter.result_is_error) \
                 ) \
            )) \
            AND ($9::TEXT IS NULL OR EXISTS ( \
                 SELECT 1 FROM timeline_file_touches ft \
                  WHERE ft.session_uuid = re.session_uuid \
                    AND ft.turn_id = re.turn_id \
                    AND ft.repo_rel_path = $9 \
            )) \
            AND ($10::TEXT IS NULL OR ( \
                 re.source_kind IN ('tool_call', 'tool_result', 'tool_error') AND o.operation_category = $10 \
            ) OR ( \
                 re.source_kind NOT IN ('tool_call', 'tool_result', 'tool_error') \
                 AND EXISTS ( \
                     SELECT 1 FROM timeline_operations ofilter \
                      WHERE ofilter.session_uuid = re.session_uuid \
                        AND ofilter.turn_id = re.turn_id \
                        AND ofilter.operation_category = $10 \
                 ) \
            )) \
            AND ($11::TEXT IS NULL OR ( \
                 re.source_kind IN ('tool_call', 'tool_result', 'tool_error') AND o.name = $11 \
            ) OR ( \
                 re.source_kind NOT IN ('tool_call', 'tool_result', 'tool_error') \
                 AND EXISTS ( \
                     SELECT 1 FROM timeline_operations ofilter \
                      WHERE ofilter.session_uuid = re.session_uuid \
                        AND ofilter.turn_id = re.turn_id \
                        AND ofilter.name = $11 \
                 ) \
            )) \
            AND ($12::BOOLEAN OR NOT ( \
                 re.source_kind IN ('tool_call', 'tool_result', 'tool_error') \
                 AND ( \
                     lower(COALESCE(o.name, '')) = ANY($13::TEXT[]) \
                     OR lower(COALESCE(o.raw_name, '')) = ANY($13::TEXT[]) \
                     OR lower(COALESCE(o.operation_type, '')) = ANY($13::TEXT[]) \
                 ) \
            )) \
          ORDER BY COALESCE(e.timestamp, tt.end_timestamp, NOW()) DESC \
          LIMIT 10000",
    )
    .bind(&include)
    .bind(scoped_session(filters))
    .bind(scoped_repo(filters))
    .bind(filters.agent.as_deref())
    .bind(filters.model.as_deref())
    .bind(filters.since)
    .bind(filters.until)
    .bind(filters.errors_only)
    .bind(filters.file_path.as_deref())
    .bind(filters.tool_category.as_deref())
    .bind(filters.tool_name.as_deref())
    .bind(filters.include_low_value)
    .bind(&low_value_tool_names)
    .fetch_all(pool)
    .await?;

    // One source can have several chunk rows; keep only its best-scoring chunk.
    let mut best: HashMap<String, SearchResult> = HashMap::new();
    for row in rows {
        let embedding: Vec<f32> = row.try_get("embedding").unwrap_or_default();
        let semantic_score = cosine_similarity(query_embedding, &embedding);
        if semantic_score < min_score {
            continue;
        }
        let source_key: String = row.try_get("source_key").unwrap_or_default();
        let mut result = match row_to_any_search_result(row, None) {
            Some(result) => result,
            None => continue,
        };
        result.semantic_score = Some(semantic_score);
        result.score = combined_score(None, Some(semantic_score));
        match best.get(&source_key) {
            Some(existing) if existing.score >= result.score => {}
            _ => {
                best.insert(source_key, result);
            }
        }
    }
    let mut scored: Vec<SearchResult> = best.into_values().collect();
    scored.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.timestamp.cmp(&left.timestamp))
    });
    scored.truncate(filters.limit as usize);
    Ok(scored)
}

fn included_source_kinds(filters: &SearchFilters) -> Vec<String> {
    filters
        .include
        .iter()
        .map(|kind| kind.as_str().to_string())
        .collect()
}

fn scoped_session(filters: &SearchFilters) -> Option<Uuid> {
    (filters.context.scope == "session")
        .then_some(filters.context.agent_session_uuid)
        .flatten()
}

fn scoped_repo(filters: &SearchFilters) -> Option<&str> {
    (filters.context.scope == "repo")
        .then_some(filters.context.repo.as_deref())
        .flatten()
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut left_norm = 0.0_f32;
    let mut right_norm = 0.0_f32;
    for (l, r) in left.iter().zip(right) {
        dot += l * r;
        left_norm += l * l;
        right_norm += r * r;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        return 0.0;
    }
    dot / (left_norm.sqrt() * right_norm.sqrt())
}
