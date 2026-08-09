use std::path::PathBuf;

use uuid::Uuid;

use crate::db::Pool;

use super::{
    fallback_session_repositories, AgentRuntimeMetadata, PtyMetaRepoMetadata, PtyMetadata,
    PtySessionRepoMetadata, PtyState, PtyWorkspaceMetadata,
};

#[derive(sqlx::FromRow)]
struct PtyRow {
    id: Uuid,
    repo: String,
    working_dir: String,
    workspace_id: Option<Uuid>,
    workspace_repo_name: Option<String>,
    workspace_kind: Option<String>,
    workspace_path: Option<String>,
    workspace_branch_name: Option<String>,
    workspace_base_ref: Option<String>,
    workspace_base_sha: Option<String>,
    workspace_merge_target: Option<String>,
    state: String,
    created_at: chrono::DateTime<chrono::Utc>,
    ended_at: Option<chrono::DateTime<chrono::Utc>>,
    exit_code: Option<i32>,
    current_session_uuid: Option<Uuid>,
    current_session_agent: Option<String>,
    agent_runtime_agent: Option<String>,
    agent_runtime_state: String,
    agent_runtime_started_at: Option<chrono::DateTime<chrono::Utc>>,
    agent_runtime_ended_at: Option<chrono::DateTime<chrono::Utc>>,
    agent_runtime_exit_code: Option<i32>,
}

impl PtyRow {
    fn into_meta(self) -> PtyMetadata {
        let workspace = self.workspace_meta();
        let repositories = fallback_session_repositories(&self.repo, workspace.as_ref());
        PtyMetadata {
            id: self.id,
            repo: self.repo,
            working_dir: PathBuf::from(self.working_dir),
            workspace,
            meta_repo: None,
            repositories,
            state: PtyState::parse(&self.state).unwrap_or(PtyState::Dead),
            created_at: self.created_at,
            ended_at: self.ended_at,
            exit_code: self.exit_code,
            current_session_uuid: self.current_session_uuid,
            current_session_agent: self.current_session_agent,
            last_event_at: None,
            label: None,
            pinned: false,
            color: None,
            agent_runtime: AgentRuntimeMetadata {
                agent: self.agent_runtime_agent,
                state: self.agent_runtime_state,
                started_at: self.agent_runtime_started_at,
                ended_at: self.agent_runtime_ended_at,
                exit_code: self.agent_runtime_exit_code,
            },
        }
    }

    fn workspace_meta(&self) -> Option<PtyWorkspaceMetadata> {
        Some(PtyWorkspaceMetadata {
            id: self.workspace_id?,
            repo_name: self.workspace_repo_name.clone()?,
            kind: self.workspace_kind.clone()?,
            path: PathBuf::from(self.workspace_path.clone()?),
            branch_name: self.workspace_branch_name.clone(),
            base_ref: self.workspace_base_ref.clone(),
            base_sha: self.workspace_base_sha.clone(),
            merge_target: self.workspace_merge_target.clone(),
        })
    }
}

/// Extended row used by `list()` with activity and user metadata.
#[derive(sqlx::FromRow)]
pub(super) struct PtyRowWithActivity {
    id: Uuid,
    repo: String,
    working_dir: String,
    workspace_id: Option<Uuid>,
    workspace_repo_name: Option<String>,
    workspace_kind: Option<String>,
    workspace_path: Option<String>,
    workspace_branch_name: Option<String>,
    workspace_base_ref: Option<String>,
    workspace_base_sha: Option<String>,
    workspace_merge_target: Option<String>,
    state: String,
    created_at: chrono::DateTime<chrono::Utc>,
    ended_at: Option<chrono::DateTime<chrono::Utc>>,
    exit_code: Option<i32>,
    current_session_uuid: Option<Uuid>,
    current_session_agent: Option<String>,
    label: Option<String>,
    pinned: bool,
    color: Option<String>,
    agent_runtime_agent: Option<String>,
    agent_runtime_state: String,
    agent_runtime_started_at: Option<chrono::DateTime<chrono::Utc>>,
    agent_runtime_ended_at: Option<chrono::DateTime<chrono::Utc>>,
    agent_runtime_exit_code: Option<i32>,
    last_event_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl PtyRowWithActivity {
    pub(super) fn into_meta(self) -> PtyMetadata {
        let workspace = self.workspace_meta();
        let repositories = fallback_session_repositories(&self.repo, workspace.as_ref());
        PtyMetadata {
            id: self.id,
            repo: self.repo,
            working_dir: PathBuf::from(self.working_dir),
            workspace,
            meta_repo: None,
            repositories,
            state: PtyState::parse(&self.state).unwrap_or(PtyState::Dead),
            created_at: self.created_at,
            ended_at: self.ended_at,
            exit_code: self.exit_code,
            current_session_uuid: self.current_session_uuid,
            current_session_agent: self.current_session_agent,
            last_event_at: self.last_event_at,
            label: self.label,
            pinned: self.pinned,
            color: self.color,
            agent_runtime: AgentRuntimeMetadata {
                agent: self.agent_runtime_agent,
                state: self.agent_runtime_state,
                started_at: self.agent_runtime_started_at,
                ended_at: self.agent_runtime_ended_at,
                exit_code: self.agent_runtime_exit_code,
            },
        }
    }

    fn workspace_meta(&self) -> Option<PtyWorkspaceMetadata> {
        Some(PtyWorkspaceMetadata {
            id: self.workspace_id?,
            repo_name: self.workspace_repo_name.clone()?,
            kind: self.workspace_kind.clone()?,
            path: PathBuf::from(self.workspace_path.clone()?),
            branch_name: self.workspace_branch_name.clone(),
            base_ref: self.workspace_base_ref.clone(),
            base_sha: self.workspace_base_sha.clone(),
            merge_target: self.workspace_merge_target.clone(),
        })
    }
}

#[derive(sqlx::FromRow)]
struct PtySessionRepoRow {
    repo_name: String,
    role: String,
    position: i32,
    workspace_id: Option<Uuid>,
    workspace_repo_name: Option<String>,
    workspace_kind: Option<String>,
    workspace_path: Option<String>,
    workspace_branch_name: Option<String>,
    workspace_base_ref: Option<String>,
    workspace_base_sha: Option<String>,
    workspace_merge_target: Option<String>,
}

impl PtySessionRepoRow {
    fn into_meta(self) -> PtySessionRepoMetadata {
        let workspace = match (
            self.workspace_id,
            self.workspace_repo_name,
            self.workspace_kind,
            self.workspace_path,
        ) {
            (Some(id), Some(repo_name), Some(kind), Some(path)) => Some(PtyWorkspaceMetadata {
                id,
                repo_name,
                kind,
                path: PathBuf::from(path),
                branch_name: self.workspace_branch_name,
                base_ref: self.workspace_base_ref,
                base_sha: self.workspace_base_sha,
                merge_target: self.workspace_merge_target,
            }),
            _ => None,
        };
        PtySessionRepoMetadata {
            repo_name: self.repo_name,
            workspace,
            role: self.role,
            position: self.position,
        }
    }
}

pub(super) async fn read_session_scope(
    pool: &Pool,
    id: Uuid,
) -> anyhow::Result<(Option<PtyMetaRepoMetadata>, Vec<PtySessionRepoMetadata>)> {
    let group: Option<(Option<Uuid>, Option<String>)> =
        sqlx::query_as("SELECT meta_repo_id, meta_repo_name FROM pty_sessions WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let meta_repo = group.and_then(|(id, name)| {
        id.map(|id| PtyMetaRepoMetadata {
            id,
            name: name.unwrap_or_else(|| "Deleted meta-repository".into()),
        })
    });
    let repositories = sqlx::query_as::<_, PtySessionRepoRow>(
        "SELECT psr.repo_name, psr.role, psr.position, \
                ws.id AS workspace_id, ws.repo_name AS workspace_repo_name, \
                ws.kind AS workspace_kind, ws.path AS workspace_path, \
                ws.branch_name AS workspace_branch_name, ws.base_ref AS workspace_base_ref, \
                ws.base_sha AS workspace_base_sha, ws.merge_target AS workspace_merge_target \
           FROM pty_session_repos psr \
           LEFT JOIN workspaces ws ON ws.id = psr.workspace_id \
          WHERE psr.pty_session_id = $1 \
          ORDER BY psr.position",
    )
    .bind(id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(PtySessionRepoRow::into_meta)
    .collect();
    Ok((meta_repo, repositories))
}

/// Read the most recent session metadata for an id directly from Postgres.
pub async fn read_meta(pool: &Pool, id: Uuid) -> anyhow::Result<Option<PtyMetadata>> {
    let row = sqlx::query_as::<_, PtyRow>(
        "SELECT ps.id, ps.repo, ps.working_dir, ps.state, ps.created_at, ps.ended_at, ps.exit_code, \
         ps.current_session_uuid, ps.current_session_agent, \
         ps.agent_runtime_agent, ps.agent_runtime_state, ps.agent_runtime_started_at, \
         ps.agent_runtime_ended_at, ps.agent_runtime_exit_code, \
         ws.id AS workspace_id, ws.repo_name AS workspace_repo_name, \
         ws.kind AS workspace_kind, ws.path AS workspace_path, \
         ws.branch_name AS workspace_branch_name, ws.base_ref AS workspace_base_ref, \
         ws.base_sha AS workspace_base_sha, ws.merge_target AS workspace_merge_target \
         FROM pty_sessions ps \
         LEFT JOIN workspaces ws ON ws.id = ps.workspace_id \
         WHERE ps.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut metadata = row.into_meta();
    let (meta_repo, repositories) = read_session_scope(pool, id).await?;
    metadata.meta_repo = meta_repo;
    if !repositories.is_empty() {
        metadata.repositories = repositories;
    }
    Ok(Some(metadata))
}
