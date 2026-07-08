use super::*;

#[derive(Debug, Deserialize)]
pub(super) struct SessionsQuery {
    repo: Option<String>,
    agent: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub(super) struct SessionView {
    agent_session_uuid: Uuid,
    agent: String,
    repo: Option<String>,
    pty_session_id: Option<Uuid>,
    model: Option<String>,
    cwd: Option<String>,
    started_at: DateTime<Utc>,
    turn_count: i64,
    total_event_count: i64,
    latest_event_at: Option<DateTime<Utc>>,
}

pub(super) async fn sessions_route(
    State(state): State<Arc<RetrievalState>>,
    Query(query): Query<SessionsQuery>,
) -> Result<Json<Vec<SessionView>>, RetrievalError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let rows = sqlx::query(
        "SELECT cs.session_uuid, cs.agent, cs.pty_session_id, \
                COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) AS repo, \
                asm.model, asm.cwd, cs.started_at, \
                COALESCE(tss.turn_count, 0)::BIGINT AS turn_count, \
                COALESCE(tss.total_event_count, 0)::BIGINT AS total_event_count, \
                tss.latest_event_at \
           FROM claude_sessions cs \
           LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
           LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
           LEFT JOIN timeline_session_state tss ON tss.session_uuid = cs.session_uuid \
          WHERE ($1::TEXT IS NULL OR COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) = $1) \
            AND ($2::TEXT IS NULL OR cs.agent = $2) \
          ORDER BY tss.latest_event_at DESC NULLS LAST, cs.started_at DESC \
          LIMIT $3",
    )
    .bind(query.repo.as_deref())
    .bind(query.agent.as_deref())
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| SessionView {
                agent_session_uuid: row.try_get("session_uuid").unwrap(),
                agent: row.try_get("agent").unwrap_or_default(),
                repo: row.try_get("repo").ok().flatten(),
                pty_session_id: row.try_get("pty_session_id").ok().flatten(),
                model: row.try_get("model").ok().flatten(),
                cwd: row.try_get("cwd").ok().flatten(),
                started_at: row.try_get("started_at").unwrap(),
                turn_count: row.try_get("turn_count").unwrap_or(0),
                total_event_count: row.try_get("total_event_count").unwrap_or(0),
                latest_event_at: row.try_get("latest_event_at").ok().flatten(),
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub(super) struct FileHistoryQuery {
    repo: String,
    path: String,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub(super) struct FileHistoryItem {
    agent_session_uuid: Uuid,
    turn_id: i64,
    timestamp: DateTime<Utc>,
    preview: String,
    touch_kind: String,
    is_write: bool,
}

pub(super) async fn file_history_route(
    State(state): State<Arc<RetrievalState>>,
    Query(query): Query<FileHistoryQuery>,
) -> Result<Json<Vec<FileHistoryItem>>, RetrievalError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let rows = sqlx::query(
        "SELECT ft.session_uuid, ft.turn_id, tt.end_timestamp, tt.preview, ft.touch_kind, ft.is_write \
           FROM timeline_file_touches ft \
           JOIN timeline_turns tt ON tt.session_uuid = ft.session_uuid AND tt.turn_id = ft.turn_id \
          WHERE ft.repo_name = $1 AND ft.repo_rel_path = $2 \
          ORDER BY tt.end_timestamp DESC \
          LIMIT $3",
    )
    .bind(&query.repo)
    .bind(&query.path)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| FileHistoryItem {
                agent_session_uuid: row.try_get("session_uuid").unwrap(),
                turn_id: row.try_get("turn_id").unwrap_or(0),
                timestamp: row.try_get("end_timestamp").unwrap(),
                preview: row.try_get("preview").unwrap_or_default(),
                touch_kind: row.try_get("touch_kind").unwrap_or_default(),
                is_write: row.try_get("is_write").unwrap_or(false),
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub(super) struct FacetsQuery {
    repo: Option<String>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub(super) struct FacetsResponse {
    agents: Vec<FacetCount>,
    operation_categories: Vec<FacetCount>,
    files: Vec<FacetCount>,
}

#[derive(Debug, Serialize)]
pub(super) struct FacetCount {
    value: String,
    count: i64,
}

pub(super) async fn facets_route(
    State(state): State<Arc<RetrievalState>>,
    Query(query): Query<FacetsQuery>,
) -> Result<Json<FacetsResponse>, RetrievalError> {
    let agent_rows = sqlx::query(
        "SELECT cs.agent AS value, COUNT(*)::BIGINT AS count \
           FROM claude_sessions cs \
           LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
           LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
          WHERE ($1::TEXT IS NULL OR COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) = $1) \
          GROUP BY cs.agent \
          ORDER BY count DESC",
    )
    .bind(query.repo.as_deref())
    .fetch_all(&state.pool)
    .await?;
    let category_rows = sqlx::query(
        "SELECT COALESCE(o.operation_category, 'other') AS value, COUNT(*)::BIGINT AS count \
           FROM timeline_operations o \
           JOIN timeline_turns tt ON tt.session_uuid = o.session_uuid AND tt.turn_id = o.turn_id \
           JOIN claude_sessions cs ON cs.session_uuid = o.session_uuid \
           LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
           LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
          WHERE ($1::TEXT IS NULL OR COALESCE(ps.repo, CASE WHEN asm.cwd LIKE '/home/dev/repos/%' THEN split_part(substr(asm.cwd, length('/home/dev/repos/') + 1), '/', 1) WHEN asm.cwd LIKE '/home/dev/workspaces/%' THEN split_part(substr(asm.cwd, length('/home/dev/workspaces/') + 1), '/', 1) ELSE NULL END) = $1) \
            AND ($2::TIMESTAMPTZ IS NULL OR tt.end_timestamp >= $2) \
            AND ($3::TIMESTAMPTZ IS NULL OR tt.end_timestamp <= $3) \
          GROUP BY COALESCE(o.operation_category, 'other') \
          ORDER BY count DESC",
    )
    .bind(query.repo.as_deref())
    .bind(query.since)
    .bind(query.until)
    .fetch_all(&state.pool)
    .await?;
    let file_rows = sqlx::query(
        "SELECT ft.repo_rel_path AS value, COUNT(*)::BIGINT AS count \
           FROM timeline_file_touches ft \
           JOIN timeline_turns tt ON tt.session_uuid = ft.session_uuid AND tt.turn_id = ft.turn_id \
          WHERE ($1::TEXT IS NULL OR ft.repo_name = $1) \
            AND ($2::TIMESTAMPTZ IS NULL OR tt.end_timestamp >= $2) \
            AND ($3::TIMESTAMPTZ IS NULL OR tt.end_timestamp <= $3) \
          GROUP BY ft.repo_rel_path \
          ORDER BY count DESC \
          LIMIT 100",
    )
    .bind(query.repo.as_deref())
    .bind(query.since)
    .bind(query.until)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(FacetsResponse {
        agents: rows_to_facets(agent_rows),
        operation_categories: rows_to_facets(category_rows),
        files: rows_to_facets(file_rows),
    }))
}

fn rows_to_facets(rows: Vec<sqlx::postgres::PgRow>) -> Vec<FacetCount> {
    rows.into_iter()
        .map(|row| FacetCount {
            value: row.try_get("value").unwrap_or_default(),
            count: row.try_get("count").unwrap_or(0),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub(super) struct TurnQuery {
    agent_session_uuid: Uuid,
    turn_id: i64,
}

#[derive(Debug, Serialize)]
pub(super) struct TurnResponse {
    agent_session_uuid: Uuid,
    turn_id: i64,
    preview: String,
    markdown: String,
    evidence: Option<EvidencePacket>,
}

pub(super) async fn turn_route(
    State(state): State<Arc<RetrievalState>>,
    Query(query): Query<TurnQuery>,
) -> Result<Json<TurnResponse>, RetrievalError> {
    let row = sqlx::query(
        "SELECT preview, markdown FROM timeline_turns WHERE session_uuid = $1 AND turn_id = $2",
    )
    .bind(query.agent_session_uuid)
    .bind(query.turn_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| RetrievalError::bad_request("turn not found"))?;
    Ok(Json(TurnResponse {
        agent_session_uuid: query.agent_session_uuid,
        turn_id: query.turn_id,
        preview: row.try_get("preview").unwrap_or_default(),
        markdown: row.try_get("markdown").unwrap_or_default(),
        evidence: load_evidence(&state.pool, query.agent_session_uuid, query.turn_id, true).await?,
    }))
}
