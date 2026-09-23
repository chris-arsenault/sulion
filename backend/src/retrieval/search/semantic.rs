use super::*;

/// ANN search over `embedding_vector` under the HNSW index. pgvector is a
/// startup requirement of the service, so this is the only path.
pub(super) async fn semantic_search(
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
                 WHEN r.source_kind = 'turn_digest' THEN tt.markdown \
                 ELSE b.text \
               END AS text, \
               o.name, o.raw_name, o.operation_type, o.operation_category, o.input, \
               o.result_content, o.result_payload, o.is_error, o.result_is_error, \
               r.semantic_score, tt.preview AS turn_preview, \
               (cs.purged_at IS NOT NULL) AS archived \
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

fn included_source_kinds(filters: &SearchFilters) -> Vec<String> {
    let mut kinds: Vec<String> = filters
        .include
        .iter()
        .map(|kind| kind.as_str().to_string())
        .collect();
    // Archived sessions embed the turn digest in place of their text blocks.
    if filters.include.iter().any(|kind| kind.covers_digest())
        && !kinds.iter().any(|kind| kind == "turn_digest")
    {
        kinds.push("turn_digest".to_string());
    }
    kinds
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
