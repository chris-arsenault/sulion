use super::*;

#[derive(Debug, Deserialize)]
pub(super) struct ContextQuery {
    pub(super) repo: Option<String>,
    pub(super) scope: Option<String>,
    pub(super) agent_session_uuid: Option<Uuid>,
    pub(super) pty_session_id: Option<Uuid>,
    pub(super) workspace_id: Option<Uuid>,
    pub(super) cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedContext {
    pub scope: String,
    pub repo: Option<String>,
    pub agent_session_uuid: Option<Uuid>,
    pub pty_session_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub cwd: Option<String>,
    pub resolution: Vec<String>,
}

pub(super) async fn context_route(
    State(state): State<Arc<RetrievalState>>,
    headers: HeaderMap,
    Query(query): Query<ContextQuery>,
) -> Result<Json<ResolvedContext>, RetrievalError> {
    let scope = Scope::parse(query.scope.as_deref())?;
    let context = resolve_context(&state.pool, &headers, query, scope).await?;
    Ok(Json(context))
}

pub(super) async fn resolve_context(
    pool: &Pool,
    headers: &HeaderMap,
    query: ContextQuery,
    scope: Scope,
) -> Result<ResolvedContext, RetrievalError> {
    let mut resolution = Vec::new();
    let mut repo = clean_opt(query.repo).or_else(|| header_string(headers, "x-sulion-repo"));
    if repo.is_some() {
        resolution.push("repo from explicit query/header".to_string());
    }

    let pty_session_id = query
        .pty_session_id
        .or_else(|| header_uuid(headers, "x-sulion-pty-id"));
    let workspace_id = query
        .workspace_id
        .or_else(|| header_uuid(headers, "x-sulion-workspace-id"));
    let agent_session_uuid = query
        .agent_session_uuid
        .or_else(|| header_uuid(headers, "x-sulion-agent-session-id"))
        .or_else(|| header_uuid(headers, "x-codex-thread-id"))
        .or_else(|| header_uuid(headers, "x-claude-session-id"));
    let cwd = clean_opt(query.cwd).or_else(|| header_string(headers, "x-sulion-cwd"));

    if repo.is_none() {
        if let Some(workspace_id) = workspace_id {
            let row: Option<(String,)> =
                sqlx::query_as("SELECT repo_name FROM workspaces WHERE id = $1")
                    .bind(workspace_id)
                    .fetch_optional(pool)
                    .await?;
            if let Some((workspace_repo,)) = row {
                repo = Some(workspace_repo);
                resolution.push("repo from workspace_id".to_string());
            }
        }
    }

    if repo.is_none() {
        if let Some(pty_session_id) = pty_session_id {
            let row: Option<(String,)> =
                sqlx::query_as("SELECT repo FROM pty_sessions WHERE id = $1")
                    .bind(pty_session_id)
                    .fetch_optional(pool)
                    .await?;
            if let Some((pty_repo,)) = row {
                repo = Some(pty_repo);
                resolution.push("repo from pty_session_id".to_string());
            }
        }
    }

    if repo.is_none() {
        if let Some(session_uuid) = agent_session_uuid {
            let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT ps.repo, asm.cwd \
                   FROM claude_sessions cs \
                   LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
                   LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
                  WHERE cs.session_uuid = $1",
            )
            .bind(session_uuid)
            .fetch_optional(pool)
            .await?;
            if let Some((session_repo, session_cwd)) = row {
                repo =
                    session_repo.or_else(|| session_cwd.as_deref().and_then(infer_repo_from_cwd));
                if repo.is_some() {
                    resolution.push("repo from agent session metadata".to_string());
                }
            }
        }
    }

    if repo.is_none() {
        repo = cwd.as_deref().and_then(infer_repo_from_cwd);
        if repo.is_some() {
            resolution.push("repo from cwd".to_string());
        }
    }

    if scope == Scope::Repo && repo.is_none() {
        return Err(RetrievalError::bad_request(
            "repo scope requires repo context; pass repo or use scope=all explicitly",
        ));
    }
    if scope == Scope::Session && agent_session_uuid.is_none() {
        return Err(RetrievalError::bad_request(
            "session scope requires agent_session_uuid",
        ));
    }

    Ok(ResolvedContext {
        scope: scope.as_str().to_string(),
        repo,
        agent_session_uuid,
        pty_session_id,
        workspace_id,
        cwd,
        resolution,
    })
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| clean_opt(Some(value.to_string())))
}

fn header_uuid(headers: &HeaderMap, name: &str) -> Option<Uuid> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
}

pub(super) fn clean_opt(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn infer_repo_from_cwd(cwd: &str) -> Option<String> {
    for prefix in ["/home/sulion/repos/", "/home/sulion/workspaces/"] {
        if let Some(rest) = cwd.strip_prefix(prefix) {
            let repo = rest.split('/').next()?.trim();
            if !repo.is_empty() {
                return Some(repo.to_string());
            }
        }
    }
    None
}
