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
    plans: Vec<crate::plans::PlanSummaryView>,
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
    agent_usage: Option<AppAgentUsageView>,
    activity: AppActivityView,
    current_plan: Option<AppCurrentPlanView>,
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

#[derive(Serialize)]
struct AppAgentUsageView {
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
    context_tokens: Option<i64>,
    model_context_window: Option<i64>,
    observed_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct AppActivityView {
    state: String,
    summary: Option<String>,
    reason: Option<String>,
    source: String,
    confidence: String,
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct AppCurrentPlanView {
    id: Uuid,
    title: String,
    status: String,
    revision: i64,
    total_phases: i32,
    completed_phases: i32,
    current_phase_id: Option<Uuid>,
    current_phase_title: Option<String>,
    current_phase_status: Option<String>,
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
    usage_agent: Option<String>,
    usage_input_tokens: Option<i64>,
    usage_cached_input_tokens: Option<i64>,
    usage_output_tokens: Option<i64>,
    usage_reasoning_output_tokens: Option<i64>,
    usage_total_tokens: Option<i64>,
    usage_context_tokens: Option<i64>,
    usage_model_context_window: Option<i64>,
    usage_observed_at: Option<DateTime<Utc>>,
    usage_updated_at: Option<DateTime<Utc>>,
    activity_state: Option<String>,
    activity_summary: Option<String>,
    activity_reason: Option<String>,
    activity_source: Option<String>,
    activity_confidence: Option<String>,
    activity_updated_at: Option<DateTime<Utc>>,
    plan_id: Option<Uuid>,
    plan_title: Option<String>,
    plan_status: Option<String>,
    plan_revision: Option<i64>,
    plan_total_phases: Option<i32>,
    plan_completed_phases: Option<i32>,
    plan_current_phase_id: Option<Uuid>,
    plan_current_phase_title: Option<String>,
    plan_current_phase_status: Option<String>,
    future_prompts_pending_count: i32,
}

impl From<AppSessionRow> for AppSessionView {
    fn from(row: AppSessionRow) -> Self {
        let workspace = row.workspace_view();
        let activity = row.activity_view();
        let current_plan = row.current_plan_view();
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
            agent_usage: row.usage_agent.map(|_| AppAgentUsageView {
                input_tokens: row.usage_input_tokens.unwrap_or_default(),
                cached_input_tokens: row.usage_cached_input_tokens.unwrap_or_default(),
                output_tokens: row.usage_output_tokens.unwrap_or_default(),
                reasoning_output_tokens: row.usage_reasoning_output_tokens.unwrap_or_default(),
                total_tokens: row.usage_total_tokens.unwrap_or_default(),
                context_tokens: row.usage_context_tokens,
                model_context_window: row
                    .usage_model_context_window
                    .or(row.metadata_model_context_window),
                observed_at: row.usage_observed_at.unwrap_or_else(Utc::now),
                updated_at: row.usage_updated_at.unwrap_or_else(Utc::now),
            }),
            activity,
            current_plan,
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

    fn activity_view(&self) -> AppActivityView {
        let runtime_active = matches!(self.agent_runtime_state.as_str(), "starting" | "running");
        if self.state != "live" || !runtime_active {
            return AppActivityView {
                state: "shell".to_string(),
                summary: None,
                reason: None,
                source: "launcher".to_string(),
                confidence: "explicit".to_string(),
                updated_at: self.agent_runtime_ended_at.or(self.ended_at),
            };
        }
        if self.agent_runtime_state == "starting" {
            return AppActivityView {
                state: "starting".to_string(),
                summary: None,
                reason: None,
                source: "launcher".to_string(),
                confidence: "explicit".to_string(),
                updated_at: self.agent_runtime_started_at,
            };
        }
        AppActivityView {
            state: self
                .activity_state
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            summary: self.activity_summary.clone(),
            reason: self.activity_reason.clone(),
            source: self
                .activity_source
                .clone()
                .unwrap_or_else(|| "launcher".to_string()),
            confidence: self
                .activity_confidence
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            updated_at: self.activity_updated_at.or(self.agent_runtime_started_at),
        }
    }

    fn current_plan_view(&self) -> Option<AppCurrentPlanView> {
        Some(AppCurrentPlanView {
            id: self.plan_id?,
            title: self.plan_title.clone()?,
            status: self.plan_status.clone()?,
            revision: self.plan_revision?,
            total_phases: self.plan_total_phases.unwrap_or_default(),
            completed_phases: self.plan_completed_phases.unwrap_or_default(),
            current_phase_id: self.plan_current_phase_id,
            current_phase_title: self.plan_current_phase_title.clone(),
            current_phase_status: self.plan_current_phase_status.clone(),
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
    let plans = crate::plans::list_open_summaries(&state.pool)
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
        plans,
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
                aus.agent AS usage_agent, aus.input_tokens AS usage_input_tokens, \
                aus.cached_input_tokens AS usage_cached_input_tokens, \
                aus.output_tokens AS usage_output_tokens, \
                aus.reasoning_output_tokens AS usage_reasoning_output_tokens, \
                aus.total_tokens AS usage_total_tokens, \
                aus.context_tokens AS usage_context_tokens, \
                aus.model_context_window AS usage_model_context_window, \
                aus.observed_at AS usage_observed_at, aus.updated_at AS usage_updated_at, \
                sas.state AS activity_state, sas.summary AS activity_summary, \
                sas.reason AS activity_reason, sas.source AS activity_source, \
                sas.confidence AS activity_confidence, sas.updated_at AS activity_updated_at, \
                plan.id AS plan_id, plan.title AS plan_title, plan.status AS plan_status, \
                plan.revision AS plan_revision, \
                COALESCE(plan_stats.total_phases, 0)::INT AS plan_total_phases, \
                COALESCE(plan_stats.completed_phases, 0)::INT AS plan_completed_phases, \
                current_phase.id AS plan_current_phase_id, \
                current_phase.title AS plan_current_phase_title, \
                current_phase.status AS plan_current_phase_status, \
                COALESCE(fps.pending_count, 0)::INT AS future_prompts_pending_count \
           FROM pty_sessions ps \
           LEFT JOIN workspaces ws ON ws.id = ps.workspace_id \
           LEFT JOIN timeline_session_state tss ON tss.session_uuid = ps.current_session_uuid \
           LEFT JOIN future_prompt_session_state fps ON fps.session_uuid = ps.current_session_uuid \
           LEFT JOIN agent_session_metadata asm ON asm.session_uuid = ps.current_session_uuid \
           LEFT JOIN agent_session_usage aus ON aus.session_uuid = ps.current_session_uuid \
           LEFT JOIN session_activity_state sas ON sas.pty_session_id = ps.id \
           LEFT JOIN plan_attachments pa \
             ON pa.pty_session_id = ps.id AND pa.detached_at IS NULL \
           LEFT JOIN plans plan ON plan.id = pa.plan_id \
           LEFT JOIN LATERAL ( \
               SELECT COUNT(*) AS total_phases, \
                      COUNT(*) FILTER (WHERE status = 'completed') AS completed_phases \
                 FROM plan_phases pp WHERE pp.plan_id = plan.id \
           ) plan_stats ON TRUE \
           LEFT JOIN LATERAL ( \
               SELECT id, title, status \
                 FROM plan_phases pp \
                WHERE pp.plan_id = plan.id \
                  AND pp.status IN ('in_progress', 'blocked') \
                ORDER BY CASE pp.status WHEN 'blocked' THEN 0 ELSE 1 END, pp.position \
                LIMIT 1 \
           ) current_phase ON TRUE \
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
