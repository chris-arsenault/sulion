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
    pub repos: Vec<String>,
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
    let explicit_repo = clean_opt(query.repo);
    let header_repos = header_repo_names(headers);
    let header_repo = header_string(headers, "x-sulion-repo");
    let mut repos = Vec::new();

    let mut pty_session_id = query
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

    if let Some(repo) = explicit_repo {
        repos.push(repo);
        resolution.push("repo from explicit query".to_string());
    } else if !header_repos.is_empty() {
        repos = header_repos;
        resolution.push("repos from collection header".to_string());
    }

    if repos.is_empty() {
        if let Some(id) = pty_session_id {
            repos = repos_for_pty(pool, id).await?;
            if !repos.is_empty() {
                resolution.push("repos from pty_session_id".to_string());
            }
        }
    }

    if repos.is_empty() {
        if let Some(workspace_id) = workspace_id {
            let row: Option<(String,)> =
                sqlx::query_as("SELECT repo_name FROM workspaces WHERE id = $1")
                    .bind(workspace_id)
                    .fetch_optional(pool)
                    .await?;
            if let Some((workspace_repo,)) = row {
                repos.push(workspace_repo);
                resolution.push("repo from workspace_id".to_string());
            }
        }
    }

    if repos.is_empty() {
        if let Some(repo) = header_repo {
            repos.push(repo);
            resolution.push("repo from header".to_string());
        }
    }

    if repos.is_empty() {
        if let Some(session_uuid) = agent_session_uuid {
            let row: Option<(Option<Uuid>, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT cs.pty_session_id, ps.repo, asm.cwd \
                   FROM claude_sessions cs \
                   LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
                   LEFT JOIN agent_session_metadata asm ON asm.session_uuid = cs.session_uuid \
                  WHERE cs.session_uuid = $1",
            )
            .bind(session_uuid)
            .fetch_optional(pool)
            .await?;
            if let Some((session_pty_id, session_repo, session_cwd)) = row {
                if let Some(id) = session_pty_id {
                    pty_session_id = Some(id);
                    repos = repos_for_pty(pool, id).await?;
                }
                if repos.is_empty() {
                    if let Some(repo) = session_repo
                        .or_else(|| session_cwd.as_deref().and_then(infer_repo_from_cwd))
                    {
                        repos.push(repo);
                    }
                }
                if !repos.is_empty() {
                    resolution.push("repos from agent session metadata".to_string());
                }
            }
        }
    }

    if repos.is_empty() {
        if let Some(repo) = cwd.as_deref().and_then(infer_repo_from_cwd) {
            repos.push(repo);
            resolution.push("repo from cwd".to_string());
        }
    }

    if scope == Scope::Repo && repos.is_empty() {
        return Err(RetrievalError::bad_request(
            "repo scope requires repo context; pass repo or use scope=all explicitly",
        ));
    }
    if scope == Scope::Session && agent_session_uuid.is_none() {
        return Err(RetrievalError::bad_request(
            "session scope requires agent_session_uuid",
        ));
    }

    let repo = repos.first().cloned();
    Ok(ResolvedContext {
        scope: scope.as_str().to_string(),
        repo,
        repos,
        agent_session_uuid,
        pty_session_id,
        workspace_id,
        cwd,
        resolution,
    })
}

async fn repos_for_pty(pool: &Pool, pty_session_id: Uuid) -> Result<Vec<String>, sqlx::Error> {
    let mut repos: Vec<String> = sqlx::query_scalar(
        "SELECT repo_name FROM pty_session_repos \
          WHERE pty_session_id = $1 ORDER BY position ASC",
    )
    .bind(pty_session_id)
    .fetch_all(pool)
    .await?;
    if repos.is_empty() {
        let fallback: Option<String> =
            sqlx::query_scalar("SELECT repo FROM pty_sessions WHERE id = $1")
                .bind(pty_session_id)
                .fetch_optional(pool)
                .await?;
        if let Some(repo) = fallback {
            repos.push(repo);
        }
    }
    Ok(repos)
}

fn header_repo_names(headers: &HeaderMap) -> Vec<String> {
    let Some(raw) = header_string(headers, "x-sulion-repos") else {
        return Vec::new();
    };
    let Ok(values) = serde_json::from_str::<Vec<String>>(&raw) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    values
        .into_iter()
        .filter_map(|value| clean_opt(Some(value)))
        .filter(|value| seen.insert(value.clone()))
        .collect()
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
