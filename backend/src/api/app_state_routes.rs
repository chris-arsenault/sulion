use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::routes::{ApiError, ApiResult};
use super::stats;
use crate::git;
use crate::repo_state::RepoGitSummary;
use crate::worktree::WorkspaceView;
use crate::AppState;

#[derive(Serialize)]
pub(super) struct AppStateResponse {
    generated_at: DateTime<Utc>,
    sessions: Vec<AppSessionView>,
    repos: Vec<AppRepoView>,
    workspaces: Vec<WorkspaceView>,
    stats: stats::StatsResponse,
}

#[derive(Serialize)]
struct AppSessionView {
    id: Uuid,
    repo: String,
    working_dir: String,
    workspace: Option<AppSessionWorkspaceView>,
    state: String,
    created_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    exit_code: Option<i32>,
    current_session_uuid: Option<Uuid>,
    current_session_agent: Option<String>,
    last_event_at: Option<DateTime<Utc>>,
    timeline_revision: i64,
    label: Option<String>,
    pinned: bool,
    color: Option<String>,
    agent_runtime: AppAgentRuntimeView,
    agent_metadata: Option<AppAgentSessionMetadataView>,
    future_prompts_pending_count: i32,
}

#[derive(Serialize)]
struct AppSessionWorkspaceView {
    id: Uuid,
    repo_name: String,
    kind: String,
    path: String,
    branch_name: Option<String>,
    base_ref: Option<String>,
    base_sha: Option<String>,
    merge_target: Option<String>,
}

#[derive(Serialize)]
struct AppAgentRuntimeView {
    agent: Option<String>,
    state: String,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    exit_code: Option<i32>,
}

#[derive(Serialize)]
struct AppAgentSessionMetadataView {
    agent: String,
    model: Option<String>,
    model_provider: Option<String>,
    reasoning_effort: Option<String>,
    cli_version: Option<String>,
    cwd: Option<String>,
    model_context_window: Option<i64>,
    updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct AppSessionRow {
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
    created_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    exit_code: Option<i32>,
    current_session_uuid: Option<Uuid>,
    current_session_agent: Option<String>,
    last_event_at: Option<DateTime<Utc>>,
    timeline_revision: i64,
    label: Option<String>,
    pinned: bool,
    color: Option<String>,
    agent_runtime_agent: Option<String>,
    agent_runtime_state: String,
    agent_runtime_started_at: Option<DateTime<Utc>>,
    agent_runtime_ended_at: Option<DateTime<Utc>>,
    agent_runtime_exit_code: Option<i32>,
    metadata_agent: Option<String>,
    metadata_model: Option<String>,
    metadata_model_provider: Option<String>,
    metadata_reasoning_effort: Option<String>,
    metadata_cli_version: Option<String>,
    metadata_cwd: Option<String>,
    metadata_model_context_window: Option<i64>,
    metadata_updated_at: Option<DateTime<Utc>>,
    future_prompts_pending_count: i32,
}

impl From<AppSessionRow> for AppSessionView {
    fn from(row: AppSessionRow) -> Self {
        let workspace = row.workspace_view();
        Self {
            id: row.id,
            repo: row.repo,
            working_dir: row.working_dir,
            workspace,
            state: row.state,
            created_at: row.created_at,
            ended_at: row.ended_at,
            exit_code: row.exit_code,
            current_session_uuid: row.current_session_uuid,
            current_session_agent: row.current_session_agent,
            last_event_at: row.last_event_at,
            timeline_revision: row.timeline_revision,
            label: row.label,
            pinned: row.pinned,
            color: row.color,
            agent_runtime: AppAgentRuntimeView {
                agent: row.agent_runtime_agent,
                state: row.agent_runtime_state,
                started_at: row.agent_runtime_started_at,
                ended_at: row.agent_runtime_ended_at,
                exit_code: row.agent_runtime_exit_code,
            },
            agent_metadata: row.metadata_agent.map(|agent| AppAgentSessionMetadataView {
                agent,
                model: row.metadata_model,
                model_provider: row.metadata_model_provider,
                reasoning_effort: row.metadata_reasoning_effort,
                cli_version: row.metadata_cli_version,
                cwd: row.metadata_cwd,
                model_context_window: row.metadata_model_context_window,
                updated_at: row.metadata_updated_at.unwrap_or_else(Utc::now),
            }),
            future_prompts_pending_count: row.future_prompts_pending_count,
        }
    }
}

impl AppSessionRow {
    fn workspace_view(&self) -> Option<AppSessionWorkspaceView> {
        Some(AppSessionWorkspaceView {
            id: self.workspace_id?,
            repo_name: self.workspace_repo_name.clone()?,
            kind: self.workspace_kind.clone()?,
            path: self.workspace_path.clone()?,
            branch_name: self.workspace_branch_name.clone(),
            base_ref: self.workspace_base_ref.clone(),
            base_sha: self.workspace_base_sha.clone(),
            merge_target: self.workspace_merge_target.clone(),
        })
    }
}

#[derive(Serialize)]
struct AppRepoView {
    name: String,
    path: String,
    exists: bool,
    timeline_revision: i64,
    git: RepoGitSummary,
}

#[derive(sqlx::FromRow)]
struct RepoStateRow {
    repo_name: String,
    path: String,
    exists: bool,
    git_revision: i64,
    branch: Option<String>,
    head_sha: Option<String>,
    head_subject: Option<String>,
    head_committed_at: Option<DateTime<Utc>>,
    recent_commits_json: Value,
    dirty_count: i32,
    untracked_count: i32,
    status_started_at: Option<DateTime<Utc>>,
    status_finished_at: Option<DateTime<Utc>>,
    status_error: Option<String>,
}

pub(super) async fn app_state(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<AppStateResponse>> {
    let sessions = load_sessions(&state.pool).await?;
    let timeline_revisions = load_repo_timeline_revisions(&state.pool).await?;
    let repos = load_repos(&state.pool, &timeline_revisions).await?;
    let workspaces = crate::worktree::load_workspace_views(&state.pool)
        .await
        .map_err(ApiError::Internal)?;
    let stats = match state.stats_cache.get().await {
        Some(stats) => stats,
        None => {
            stats::sample_stats_once(&state)
                .await
                .map_err(ApiError::Internal)?;
            state.stats_cache.get().await.ok_or_else(|| {
                ApiError::Internal(anyhow::anyhow!("stats cache unavailable after sample"))
            })?
        }
    };

    Ok(Json(AppStateResponse {
        generated_at: Utc::now(),
        sessions,
        repos,
        workspaces,
        stats,
    }))
}

async fn load_sessions(pool: &crate::db::Pool) -> ApiResult<Vec<AppSessionView>> {
    let rows: Vec<AppSessionRow> = sqlx::query_as(
        "SELECT ps.id, ps.repo, ps.working_dir, ps.state, ps.created_at, \
                ps.ended_at, ps.exit_code, ps.current_session_uuid, ps.current_session_agent, \
                ws.id AS workspace_id, ws.repo_name AS workspace_repo_name, \
                ws.kind AS workspace_kind, ws.path AS workspace_path, \
                ws.branch_name AS workspace_branch_name, ws.base_ref AS workspace_base_ref, \
                ws.base_sha AS workspace_base_sha, ws.merge_target AS workspace_merge_target, \
                tss.latest_event_at AS last_event_at, \
                COALESCE(tss.revision, 0)::BIGINT AS timeline_revision, \
                ps.label, ps.pinned, ps.color, \
                ps.agent_runtime_agent, ps.agent_runtime_state, ps.agent_runtime_started_at, \
                ps.agent_runtime_ended_at, ps.agent_runtime_exit_code, \
                asm.agent AS metadata_agent, asm.model AS metadata_model, \
                asm.model_provider AS metadata_model_provider, \
                asm.reasoning_effort AS metadata_reasoning_effort, \
                asm.cli_version AS metadata_cli_version, asm.cwd AS metadata_cwd, \
                asm.model_context_window AS metadata_model_context_window, \
                asm.updated_at AS metadata_updated_at, \
                COALESCE(fps.pending_count, 0)::INT AS future_prompts_pending_count \
           FROM pty_sessions ps \
           LEFT JOIN workspaces ws ON ws.id = ps.workspace_id \
           LEFT JOIN timeline_session_state tss ON tss.session_uuid = ps.current_session_uuid \
           LEFT JOIN future_prompt_session_state fps ON fps.session_uuid = ps.current_session_uuid \
           LEFT JOIN agent_session_metadata asm ON asm.session_uuid = ps.current_session_uuid \
          WHERE ps.state <> 'deleted' \
          ORDER BY ps.pinned DESC, ps.created_at DESC",
    )
    .fetch_all(pool)
    .await
    .map_err(ApiError::Db)?;
    Ok(rows.into_iter().map(AppSessionView::from).collect())
}

async fn load_repo_timeline_revisions(pool: &crate::db::Pool) -> ApiResult<HashMap<String, i64>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT ps.repo, COALESCE(SUM(tss.revision), 0)::BIGINT AS timeline_revision \
           FROM pty_sessions ps \
           LEFT JOIN timeline_session_state tss ON tss.session_uuid = ps.current_session_uuid \
          WHERE ps.state <> 'deleted' \
          GROUP BY ps.repo",
    )
    .fetch_all(pool)
    .await
    .map_err(ApiError::Db)?;
    Ok(rows.into_iter().collect())
}

async fn load_repos(
    pool: &crate::db::Pool,
    timeline_revisions: &HashMap<String, i64>,
) -> ApiResult<Vec<AppRepoView>> {
    let rows: Vec<RepoStateRow> = sqlx::query_as(
        "SELECT repo_name, path, exists, git_revision, branch, head_sha, head_subject, \
                head_committed_at, recent_commits_json, dirty_count, untracked_count, \
                status_started_at, status_finished_at, status_error \
           FROM repo_runtime_state \
          WHERE exists = TRUE \
          ORDER BY repo_name ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(ApiError::Db)?;

    rows.into_iter()
        .map(|row| {
            let git = repo_git_summary(&row)?;
            Ok(AppRepoView {
                timeline_revision: timeline_revisions
                    .get(&row.repo_name)
                    .copied()
                    .unwrap_or_default(),
                name: row.repo_name,
                path: row.path,
                exists: row.exists,
                git,
            })
        })
        .collect()
}

fn repo_git_summary(row: &RepoStateRow) -> ApiResult<RepoGitSummary> {
    let mut recent_commits =
        serde_json::from_value::<Vec<git::Commit>>(row.recent_commits_json.clone())
            .context("deserialize repo recent commits")
            .map_err(ApiError::Internal)?;
    let last_commit = match (
        row.head_sha.as_ref(),
        row.head_subject.as_ref(),
        row.head_committed_at,
    ) {
        (Some(sha), Some(subject), Some(committed_at)) => Some(git::Commit {
            sha: sha.clone(),
            subject: subject.clone(),
            committed_at: committed_at.to_rfc3339(),
        }),
        _ => recent_commits.first().cloned(),
    };
    if recent_commits.is_empty() {
        if let Some(commit) = last_commit.clone() {
            recent_commits.push(commit);
        }
    }
    let refreshing = match (row.status_started_at, row.status_finished_at) {
        (Some(started), Some(finished)) => started > finished,
        (Some(_), None) => true,
        _ => false,
    };

    Ok(RepoGitSummary {
        revision: row.git_revision,
        branch: row.branch.clone(),
        uncommitted_count: row.dirty_count,
        untracked_count: row.untracked_count,
        last_commit,
        recent_commits,
        refreshing,
        status_error: row.status_error.clone(),
    })
}
