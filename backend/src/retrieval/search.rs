use super::*;

mod types;

pub(super) use types::EvidencePacket;
use types::*;

pub(super) async fn search_route(
    State(state): State<Arc<RetrievalState>>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, RetrievalError> {
    let response = search_inner(&state, headers, query).await?;
    Ok(Json(response))
}

async fn search_inner(
    state: &RetrievalState,
    headers: HeaderMap,
    query: SearchQuery,
) -> Result<SearchResponse, RetrievalError> {
    let q = query.q.trim().to_string();
    if q.is_empty() {
        return Err(RetrievalError::bad_request("q must not be empty"));
    }
    let scope = Scope::parse(query.scope.as_deref())?;
    let search_mode = SearchMode::parse(query.search_mode.as_deref())?;
    let tool_category = clean_opt(query.tool_category);
    let tool_name = clean_opt(query.tool_name);
    let include_raw = query.include.as_deref().or_else(|| {
        if tool_category.is_some() || tool_name.is_some() {
            Some("tools")
        } else {
            None
        }
    });
    let include = parse_includes(include_raw)?;
    let limit = query.limit.unwrap_or(20).clamp(1, MAX_SEARCH_LIMIT);
    let context_query = ContextQuery {
        repo: query.repo.clone(),
        scope: query.scope.clone(),
        agent_session_uuid: query.agent_session_uuid,
        pty_session_id: query.pty_session_id,
        workspace_id: query.workspace_id,
        cwd: query.cwd.clone(),
    };
    let context = resolve_context(&state.pool, &headers, context_query, scope).await?;

    let filters = SearchFilters {
        context: context.clone(),
        include,
        file_path: clean_opt(query.file_path),
        tool_category,
        tool_name,
        agent: clean_opt(query.agent),
        model: clean_opt(query.model),
        errors_only: query.errors_only.unwrap_or(false),
        since: query.since,
        until: query.until,
        limit,
    };

    let mut warnings = Vec::new();
    let mut merged: HashMap<ResultKey, SearchResult> = HashMap::new();

    if matches!(search_mode, SearchMode::Hybrid | SearchMode::Lexical) {
        for result in lexical_search(&state.pool, &q, &filters).await? {
            merged.insert(ResultKey::from_result(&result), result);
        }
    }

    if matches!(search_mode, SearchMode::Hybrid | SearchMode::Semantic) {
        let pending_sources = super::embeddings::pending_source_count(state).await?;
        if pending_sources > 0 {
            warnings.push(format!(
                "{pending_sources} retrieval sources are pending semantic indexing; semantic results may be stale"
            ));
        }
        let embedder = state.embedding_client();
        let query_embedding = match embedder.embed_one(&q).await {
            Ok(embedding) => embedding,
            Err(err) if search_mode == SearchMode::Hybrid => {
                warnings.push(format!("semantic search skipped: {err}"));
                let mut results = merged.into_values().collect::<Vec<_>>();
                finalize_results(&state.pool, &mut results, limit).await?;
                return Ok(SearchResponse {
                    context,
                    search_mode: search_mode.as_str().to_string(),
                    warnings,
                    results,
                });
            }
            Err(err) => return Err(err),
        };
        let semantic = semantic_search(state, &query_embedding, &filters).await?;
        for result in semantic {
            let key = ResultKey::from_result(&result);
            match merged.get_mut(&key) {
                Some(existing) => {
                    existing.semantic_score = result.semantic_score;
                    existing.score =
                        combined_score(existing.lexical_score, existing.semantic_score);
                }
                None => {
                    merged.insert(key, result);
                }
            }
        }
    }

    let mut results = merged.into_values().collect::<Vec<_>>();
    finalize_results(&state.pool, &mut results, limit).await?;
    Ok(SearchResponse {
        context,
        search_mode: search_mode.as_str().to_string(),
        warnings,
        results,
    })
}

async fn lexical_search(
    pool: &Pool,
    q: &str,
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, RetrievalError> {
    let include = filters
        .include
        .iter()
        .map(|kind| kind.as_str().to_string())
        .collect::<Vec<_>>();
    let mut results = Vec::new();
    let rows = sqlx::query(
        "WITH candidates AS ( \
             SELECT \
                CASE \
                  WHEN e.speaker = 'assistant' AND b.kind = 'text' THEN 'assistant_text' \
                  WHEN e.speaker = 'user' AND b.kind = 'text' THEN 'user_prompt' \
                  WHEN e.speaker = 'summary' AND b.kind = 'text' THEN 'summary' \
                  ELSE NULL \
                END AS source_kind, \
                e.session_uuid, e.byte_offset, b.ord AS block_ord, e.timestamp, \
                cs.agent, cs.pty_session_id, ps.repo AS pty_repo, asm.cwd, asm.model, \
                b.text, \
                (similarity(b.text, $1) + \
                 CASE WHEN octet_length(b.text) <= 1000000 \
                      THEN COALESCE(ts_rank_cd(to_tsvector('simple', b.text), plainto_tsquery('simple', $1)), 0) \
                      ELSE 0 \
                 END)::REAL AS lexical_score, \
                tt.turn_id, tt.preview AS turn_preview \
             FROM event_blocks b \
             JOIN events e ON e.session_uuid = b.session_uuid AND e.byte_offset = b.byte_offset \
             JOIN claude_sessions cs ON cs.session_uuid = e.session_uuid \
             LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
             LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
             LEFT JOIN LATERAL ( \
                SELECT turn_id, preview \
                  FROM timeline_turns tt \
                 WHERE tt.session_uuid = e.session_uuid \
                   AND tt.start_timestamp <= e.timestamp \
                   AND tt.end_timestamp >= e.timestamp \
                 ORDER BY tt.duration_ms ASC, tt.turn_id ASC \
                 LIMIT 1 \
             ) tt ON TRUE \
             WHERE b.kind = 'text' \
               AND b.text IS NOT NULL \
               AND b.text ILIKE '%' || $1 || '%' \
               AND ($2::UUID IS NULL OR e.session_uuid = $2) \
               AND ($3::TEXT IS NULL OR COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) = $3) \
               AND ($4::TEXT IS NULL OR cs.agent = $4) \
               AND ($5::TEXT IS NULL OR asm.model = $5) \
               AND ($6::TIMESTAMPTZ IS NULL OR e.timestamp >= $6) \
               AND ($7::TIMESTAMPTZ IS NULL OR e.timestamp <= $7) \
        ) \
        SELECT * \
          FROM candidates c \
         WHERE source_kind = ANY($8) \
           AND ($9::BOOLEAN = FALSE OR EXISTS ( \
                SELECT 1 FROM timeline_operations o \
                 WHERE o.session_uuid = c.session_uuid \
                   AND o.turn_id = c.turn_id \
                   AND (o.is_error OR o.result_is_error) \
           )) \
           AND ($10::TEXT IS NULL OR EXISTS ( \
                SELECT 1 FROM timeline_file_touches ft \
                 WHERE ft.session_uuid = c.session_uuid \
                   AND ft.turn_id = c.turn_id \
                   AND ft.repo_rel_path = $10 \
           )) \
           AND ($11::TEXT IS NULL OR EXISTS ( \
                SELECT 1 FROM timeline_operations o \
                 WHERE o.session_uuid = c.session_uuid \
                   AND o.turn_id = c.turn_id \
                   AND o.operation_category = $11 \
           )) \
           AND ($12::TEXT IS NULL OR EXISTS ( \
                SELECT 1 FROM timeline_operations o \
                 WHERE o.session_uuid = c.session_uuid \
                   AND o.turn_id = c.turn_id \
                   AND o.name = $12 \
           )) \
         ORDER BY lexical_score DESC, timestamp DESC \
         LIMIT $13",
    )
    .bind(q)
    .bind(if filters.context.scope == "session" {
        filters.context.agent_session_uuid
    } else {
        None
    })
    .bind(if filters.context.scope == "repo" {
        filters.context.repo.as_deref()
    } else {
        None
    })
    .bind(filters.agent.as_deref())
    .bind(filters.model.as_deref())
    .bind(filters.since)
    .bind(filters.until)
    .bind(&include)
    .bind(filters.errors_only)
    .bind(filters.file_path.as_deref())
    .bind(filters.tool_category.as_deref())
    .bind(filters.tool_name.as_deref())
    .bind(filters.limit)
    .fetch_all(pool)
    .await?;

    results.extend(
        rows.into_iter()
            .filter_map(|row| row_to_search_result(row, Some("lexical"))),
    );

    if filters.include.iter().any(|kind| kind.is_tool()) {
        results.extend(lexical_tool_search(pool, q, filters).await?);
    }

    Ok(results)
}

async fn lexical_tool_search(
    pool: &Pool,
    q: &str,
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, RetrievalError> {
    let include = filters
        .include
        .iter()
        .filter(|kind| kind.is_tool())
        .map(|kind| kind.as_str().to_string())
        .collect::<Vec<_>>();
    if include.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "WITH tool_sources AS ( \
             SELECT \
                'tool_call' AS source_kind, \
                o.session_uuid, o.turn_id, o.operation_ord, tt.end_timestamp AS timestamp, \
                cs.agent, cs.pty_session_id, ps.repo AS pty_repo, asm.cwd, asm.model, \
                concat_ws(' ', o.name, o.raw_name, o.operation_type, o.operation_category, o.input::TEXT) AS text, \
                o.name, o.raw_name, o.operation_type, o.operation_category, o.input, \
                o.result_content, o.result_payload, o.is_error, o.result_is_error, tt.preview AS turn_preview \
             FROM timeline_operations o \
             JOIN timeline_turns tt ON tt.session_uuid = o.session_uuid AND tt.turn_id = o.turn_id \
             JOIN claude_sessions cs ON cs.session_uuid = o.session_uuid \
             LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
             LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
             WHERE o.input IS NOT NULL \
             UNION ALL \
             SELECT \
                CASE WHEN o.result_is_error OR o.is_error THEN 'tool_error' ELSE 'tool_result' END AS source_kind, \
                o.session_uuid, o.turn_id, o.operation_ord, tt.end_timestamp AS timestamp, \
                cs.agent, cs.pty_session_id, ps.repo AS pty_repo, asm.cwd, asm.model, \
                concat_ws(' ', o.name, o.result_content, o.result_payload::TEXT) AS text, \
                o.name, o.raw_name, o.operation_type, o.operation_category, o.input, \
                o.result_content, o.result_payload, o.is_error, o.result_is_error, tt.preview AS turn_preview \
             FROM timeline_operations o \
             JOIN timeline_turns tt ON tt.session_uuid = o.session_uuid AND tt.turn_id = o.turn_id \
             JOIN claude_sessions cs ON cs.session_uuid = o.session_uuid \
             LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
             LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
             WHERE o.result_content IS NOT NULL OR o.result_payload IS NOT NULL OR o.result_is_error OR o.is_error \
        ), candidates AS ( \
             SELECT *, \
                (similarity(text, $1) + \
                 CASE WHEN octet_length(text) <= 1000000 \
                      THEN COALESCE(ts_rank_cd(to_tsvector('simple', text), plainto_tsquery('simple', $1)), 0) \
                      ELSE 0 \
                 END)::REAL AS lexical_score \
               FROM tool_sources \
              WHERE length(trim(text)) > 0 \
                AND text ILIKE '%' || $1 || '%' \
                AND source_kind = ANY($2) \
                AND ($3::UUID IS NULL OR session_uuid = $3) \
                AND ($4::TEXT IS NULL OR COALESCE(pty_repo, CASE WHEN cwd LIKE '/home/dev/repos/%' THEN split_part(substr(cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) = $4) \
                AND ($5::TEXT IS NULL OR agent = $5) \
                AND ($6::TEXT IS NULL OR model = $6) \
                AND ($7::TIMESTAMPTZ IS NULL OR timestamp >= $7) \
                AND ($8::TIMESTAMPTZ IS NULL OR timestamp <= $8) \
                AND ($9::BOOLEAN = FALSE OR is_error OR result_is_error) \
                AND ($10::TEXT IS NULL OR EXISTS ( \
                     SELECT 1 FROM timeline_file_touches ft \
                      WHERE ft.session_uuid = tool_sources.session_uuid \
                        AND ft.turn_id = tool_sources.turn_id \
                        AND ft.repo_rel_path = $10 \
                )) \
                AND ($11::TEXT IS NULL OR operation_category = $11) \
                AND ($12::TEXT IS NULL OR name = $12) \
        ) \
        SELECT * FROM candidates \
         ORDER BY lexical_score DESC, timestamp DESC \
         LIMIT $13",
    )
    .bind(q)
    .bind(&include)
    .bind(if filters.context.scope == "session" {
        filters.context.agent_session_uuid
    } else {
        None
    })
    .bind(if filters.context.scope == "repo" {
        filters.context.repo.as_deref()
    } else {
        None
    })
    .bind(filters.agent.as_deref())
    .bind(filters.model.as_deref())
    .bind(filters.since)
    .bind(filters.until)
    .bind(filters.errors_only)
    .bind(filters.file_path.as_deref())
    .bind(filters.tool_category.as_deref())
    .bind(filters.tool_name.as_deref())
    .bind(filters.limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|row| row_to_tool_search_result(row, Some("lexical")))
        .collect())
}

fn row_to_search_result(row: sqlx::postgres::PgRow, source: Option<&str>) -> Option<SearchResult> {
    let source_kind: Option<String> = row.try_get("source_kind").ok()?;
    let source_kind = source_kind?;
    let lexical_score = match source {
        Some("lexical") => row.try_get::<f32, _>("lexical_score").ok(),
        _ => None,
    };
    let semantic_score = match source {
        Some("semantic") => row.try_get::<f32, _>("semantic_score").ok(),
        _ => None,
    };
    let text: String = row.try_get("text").unwrap_or_default();
    let snippet = snippet(&text);
    let preview: Option<String> = row.try_get("turn_preview").ok();
    let repo = row
        .try_get::<Option<String>, _>("pty_repo")
        .unwrap_or_default()
        .or_else(|| {
            row.try_get::<Option<String>, _>("cwd")
                .unwrap_or_default()
                .as_deref()
                .and_then(infer_repo_from_cwd)
        });
    Some(SearchResult {
        source_kind,
        score: combined_score(lexical_score, semantic_score),
        lexical_score,
        semantic_score,
        repo,
        agent_session_uuid: row.try_get("session_uuid").ok()?,
        agent: row.try_get("agent").ok()?,
        pty_session_id: row.try_get("pty_session_id").ok().flatten(),
        turn_id: row.try_get("turn_id").ok().flatten(),
        operation_ord: row.try_get("operation_ord").ok().flatten(),
        byte_offset: row.try_get("byte_offset").ok(),
        block_ord: row.try_get("block_ord").ok(),
        timestamp: row.try_get("timestamp").ok(),
        preview: preview.unwrap_or_else(|| snippet.clone()),
        snippet,
        tool: None,
        evidence: None,
    })
}

fn row_to_any_search_result(
    row: sqlx::postgres::PgRow,
    source: Option<&str>,
) -> Option<SearchResult> {
    let source_kind: Option<String> = row.try_get("source_kind").ok()?;
    match source_kind.as_deref()? {
        "tool_call" | "tool_result" | "tool_error" => row_to_tool_search_result(row, source),
        _ => row_to_search_result(row, source),
    }
}

fn row_to_tool_search_result(
    row: sqlx::postgres::PgRow,
    source: Option<&str>,
) -> Option<SearchResult> {
    let source_kind: String = row.try_get("source_kind").ok()?;
    let lexical_score = match source {
        Some("lexical") => row.try_get::<f32, _>("lexical_score").ok(),
        _ => None,
    };
    let semantic_score = match source {
        Some("semantic") => row.try_get::<f32, _>("semantic_score").ok(),
        _ => None,
    };
    let text: String = row.try_get("text").unwrap_or_default();
    let snippet = snippet(&text);
    let preview: Option<String> = row.try_get("turn_preview").ok();
    let repo = row
        .try_get::<Option<String>, _>("pty_repo")
        .unwrap_or_default()
        .or_else(|| {
            row.try_get::<Option<String>, _>("cwd")
                .unwrap_or_default()
                .as_deref()
                .and_then(infer_repo_from_cwd)
        });
    Some(SearchResult {
        source_kind,
        score: combined_score(lexical_score, semantic_score),
        lexical_score,
        semantic_score,
        repo,
        agent_session_uuid: row.try_get("session_uuid").ok()?,
        agent: row.try_get("agent").ok()?,
        pty_session_id: row.try_get("pty_session_id").ok().flatten(),
        turn_id: row.try_get("turn_id").ok().flatten(),
        operation_ord: row.try_get("operation_ord").ok().flatten(),
        byte_offset: None,
        block_ord: None,
        timestamp: row.try_get("timestamp").ok(),
        preview: preview.unwrap_or_else(|| snippet.clone()),
        snippet,
        tool: Some(ToolSearchPayload {
            name: row.try_get("name").unwrap_or_default(),
            raw_name: row.try_get("raw_name").ok().flatten(),
            operation_type: row.try_get("operation_type").ok().flatten(),
            operation_category: row.try_get("operation_category").ok().flatten(),
            input: row.try_get("input").ok().flatten(),
            result_content: row.try_get("result_content").ok().flatten(),
            result_payload: row.try_get("result_payload").ok().flatten(),
            is_error: row.try_get::<bool, _>("is_error").unwrap_or(false)
                || row.try_get::<bool, _>("result_is_error").unwrap_or(false),
        }),
        evidence: None,
    })
}

fn snippet(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 320;
    if text.chars().count() <= MAX {
        return text;
    }
    let mut out = text.chars().take(MAX).collect::<String>();
    out.push_str("...");
    out
}

fn combined_score(lexical: Option<f32>, semantic: Option<f32>) -> f32 {
    match (lexical, semantic) {
        (Some(l), Some(s)) => (l * 0.45) + (s * 0.55),
        (Some(l), None) => l,
        (None, Some(s)) => s,
        (None, None) => 0.0,
    }
}

async fn semantic_search(
    state: &RetrievalState,
    query_embedding: &[f32],
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, RetrievalError> {
    let capabilities = *state.vector_capabilities.read().await;
    if capabilities.column_exists {
        semantic_search_pgvector(state, query_embedding, filters).await
    } else {
        semantic_search_exact(&state.pool, query_embedding, filters).await
    }
}

async fn semantic_search_pgvector(
    state: &RetrievalState,
    query_embedding: &[f32],
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, RetrievalError> {
    let include = filters
        .include
        .iter()
        .map(|kind| kind.as_str().to_string())
        .collect::<Vec<_>>();
    let vector = vector_literal(query_embedding);
    let rows = sqlx::query(
        "WITH ranked AS ( \
             SELECT re.source_kind, re.session_uuid, re.byte_offset, re.block_ord, re.turn_id, re.operation_ord, \
                    re.repo_name, (1.0 - (re.embedding_vector <=> $1::vector))::REAL AS semantic_score \
               FROM retrieval_embeddings re \
              WHERE re.embedding_model = $2 \
                AND re.embedding_dimensions = $3 \
                AND re.embedding_vector IS NOT NULL \
                AND re.source_kind = ANY($4) \
                AND ($5::UUID IS NULL OR re.session_uuid = $5) \
                AND ($6::TEXT IS NULL OR re.repo_name = $6) \
              ORDER BY re.embedding_vector <=> $1::vector \
              LIMIT $7 \
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
    .bind(if filters.context.scope == "session" {
        filters.context.agent_session_uuid
    } else {
        None
    })
    .bind(if filters.context.scope == "repo" {
        filters.context.repo.as_deref()
    } else {
        None
    })
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
) -> Result<Vec<SearchResult>, RetrievalError> {
    let include = filters
        .include
        .iter()
        .map(|kind| kind.as_str().to_string())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        "SELECT re.source_kind, re.session_uuid, re.byte_offset, re.block_ord, re.turn_id, re.operation_ord, \
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
          ORDER BY COALESCE(e.timestamp, tt.end_timestamp, NOW()) DESC \
          LIMIT 10000",
    )
    .bind(&include)
    .bind(if filters.context.scope == "session" {
        filters.context.agent_session_uuid
    } else {
        None
    })
    .bind(if filters.context.scope == "repo" {
        filters.context.repo.as_deref()
    } else {
        None
    })
    .bind(filters.agent.as_deref())
    .bind(filters.model.as_deref())
    .bind(filters.since)
    .bind(filters.until)
    .bind(filters.errors_only)
    .bind(filters.file_path.as_deref())
    .bind(filters.tool_category.as_deref())
    .bind(filters.tool_name.as_deref())
    .fetch_all(pool)
    .await?;

    let mut scored = Vec::new();
    for row in rows {
        let embedding: Vec<f32> = row.try_get("embedding").unwrap_or_default();
        let semantic_score = cosine_similarity(query_embedding, &embedding);
        let mut result = match row_to_any_search_result(row, None) {
            Some(result) => result,
            None => continue,
        };
        result.semantic_score = Some(semantic_score);
        result.score = combined_score(None, Some(semantic_score));
        scored.push(result);
    }
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

async fn finalize_results(
    pool: &Pool,
    results: &mut Vec<SearchResult>,
    limit: i64,
) -> Result<(), RetrievalError> {
    results.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.timestamp.cmp(&left.timestamp))
    });
    results.truncate(limit as usize);
    for result in results {
        if let Some(turn_id) = result.turn_id {
            result.evidence = load_evidence(pool, result.agent_session_uuid, turn_id).await?;
        }
    }
    Ok(())
}

pub(super) async fn load_evidence(
    pool: &Pool,
    session_uuid: Uuid,
    turn_id: i64,
) -> Result<Option<EvidencePacket>, RetrievalError> {
    let turn = sqlx::query(
        "SELECT preview, start_timestamp, end_timestamp \
           FROM timeline_turns \
          WHERE session_uuid = $1 AND turn_id = $2",
    )
    .bind(session_uuid)
    .bind(turn_id)
    .fetch_optional(pool)
    .await?;
    let Some(turn) = turn else {
        return Ok(None);
    };
    let operation_rows = sqlx::query(
        "SELECT name, operation_category, operation_type, is_error, result_is_error \
           FROM timeline_operations \
          WHERE session_uuid = $1 AND turn_id = $2 \
          ORDER BY operation_ord ASC \
          LIMIT 24",
    )
    .bind(session_uuid)
    .bind(turn_id)
    .fetch_all(pool)
    .await?;
    let file_rows = sqlx::query(
        "SELECT repo_name, repo_rel_path, touch_kind, is_write \
           FROM timeline_file_touches \
          WHERE session_uuid = $1 AND turn_id = $2 \
          ORDER BY touch_ord ASC \
          LIMIT 48",
    )
    .bind(session_uuid)
    .bind(turn_id)
    .fetch_all(pool)
    .await?;
    Ok(Some(EvidencePacket {
        turn_preview: turn.try_get("preview").ok(),
        turn_start_timestamp: turn.try_get("start_timestamp").ok(),
        turn_end_timestamp: turn.try_get("end_timestamp").ok(),
        operations: operation_rows
            .into_iter()
            .map(|row| EvidenceOperation {
                name: row.try_get("name").unwrap_or_default(),
                operation_category: row.try_get("operation_category").ok().flatten(),
                operation_type: row.try_get("operation_type").ok().flatten(),
                is_error: row.try_get::<bool, _>("is_error").unwrap_or(false)
                    || row.try_get::<bool, _>("result_is_error").unwrap_or(false),
            })
            .collect(),
        file_touches: file_rows
            .into_iter()
            .map(|row| EvidenceFileTouch {
                repo: row.try_get("repo_name").unwrap_or_default(),
                path: row.try_get("repo_rel_path").unwrap_or_default(),
                touch_kind: row.try_get("touch_kind").unwrap_or_default(),
                is_write: row.try_get("is_write").unwrap_or(false),
            })
            .collect(),
    }))
}
