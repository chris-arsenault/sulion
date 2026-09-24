//! Workspace reads: records, API views and dirty paths.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::db::Pool;
use crate::git::{self, DiffStat};
use crate::repo_state::RepoGitSummary;

use super::{WorkspaceDirtyPaths, WorkspaceRecord, WorkspaceView};

pub async fn load_workspace(pool: &Pool, id: Uuid) -> anyhow::Result<WorkspaceRecord> {
    let row = sqlx::query_as::<_, WorkspaceRecordRow>(
        "SELECT id, repo_name, kind, path, branch_name, base_ref, base_sha, merge_target, \
                created_by_session_id, state, created_at, updated_at \
           FROM workspaces \
          WHERE id = $1 AND state <> 'deleted'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("load workspace {id}"))?
    .ok_or_else(|| anyhow::anyhow!("workspace not found: {id}"))?;
    Ok(row.into_record())
}

pub async fn load_workspace_views(pool: &Pool) -> anyhow::Result<Vec<WorkspaceView>> {
    let rows = workspace_view_rows(
        pool,
        "SELECT id, repo_name, kind, path, branch_name, base_ref, base_sha, merge_target, \
                created_by_session_id, state, git_revision, head_sha, head_subject, head_committed_at, \
                recent_commits_json, dirty_count, untracked_count, status_started_at, \
                status_finished_at, status_error, created_at, updated_at \
           FROM workspaces \
          WHERE state <> 'deleted' \
          ORDER BY repo_name ASC, kind ASC, created_at ASC",
    )
    .await?;
    rows.into_iter().map(WorkspaceViewRow::into_view).collect()
}

pub async fn load_workspace_view(pool: &Pool, id: Uuid) -> anyhow::Result<WorkspaceView> {
    let row = sqlx::query_as::<_, WorkspaceViewRow>(
        "SELECT id, repo_name, kind, path, branch_name, base_ref, base_sha, merge_target, \
                created_by_session_id, state, git_revision, head_sha, head_subject, head_committed_at, \
                recent_commits_json, dirty_count, untracked_count, status_started_at, \
                status_finished_at, status_error, created_at, updated_at \
           FROM workspaces \
          WHERE id = $1 AND state <> 'deleted'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("load workspace view {id}"))?
    .ok_or_else(|| anyhow::anyhow!("workspace not found: {id}"))?;
    row.into_view()
}

async fn workspace_view_rows(pool: &Pool, sql: &str) -> anyhow::Result<Vec<WorkspaceViewRow>> {
    sqlx::query_as::<_, WorkspaceViewRow>(sql)
        .fetch_all(pool)
        .await
        .context("load workspace views")
}

pub async fn load_workspace_dirty_paths(
    pool: &Pool,
    workspace_id: Uuid,
) -> anyhow::Result<WorkspaceDirtyPaths> {
    let (git_revision,): (i64,) =
        sqlx::query_as("SELECT git_revision FROM workspaces WHERE id = $1 AND state = 'active'")
            .bind(workspace_id)
            .fetch_one(pool)
            .await
            .with_context(|| format!("load git revision for workspace {workspace_id}"))?;

    let rows: Vec<(String, String, Option<i32>, Option<i32>)> = sqlx::query_as(
        "SELECT path, status, additions, deletions \
           FROM workspace_dirty_paths \
          WHERE workspace_id = $1 \
          ORDER BY path ASC",
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("load dirty paths for workspace {workspace_id}"))?;

    let mut dirty_by_path = HashMap::new();
    let mut diff_stats_by_path = HashMap::new();
    for (path, status, additions, deletions) in rows {
        dirty_by_path.insert(path.clone(), status);
        if let (Some(additions), Some(deletions)) = (additions, deletions) {
            diff_stats_by_path.insert(
                path,
                DiffStat {
                    additions: additions.max(0) as usize,
                    deletions: deletions.max(0) as usize,
                },
            );
        }
    }

    Ok(WorkspaceDirtyPaths {
        workspace_id,
        git_revision,
        dirty_by_path,
        diff_stats_by_path,
    })
}

#[derive(sqlx::FromRow)]
struct WorkspaceRecordRow {
    id: Uuid,
    repo_name: String,
    kind: String,
    path: String,
    branch_name: Option<String>,
    base_ref: Option<String>,
    base_sha: Option<String>,
    merge_target: Option<String>,
    created_by_session_id: Option<Uuid>,
    state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl WorkspaceRecordRow {
    fn into_record(self) -> WorkspaceRecord {
        WorkspaceRecord {
            id: self.id,
            repo_name: self.repo_name,
            kind: self.kind,
            path: PathBuf::from(self.path),
            branch_name: self.branch_name,
            base_ref: self.base_ref,
            base_sha: self.base_sha,
            merge_target: self.merge_target,
            created_by_session_id: self.created_by_session_id,
            state: self.state,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct WorkspaceViewRow {
    id: Uuid,
    repo_name: String,
    kind: String,
    path: String,
    branch_name: Option<String>,
    base_ref: Option<String>,
    base_sha: Option<String>,
    merge_target: Option<String>,
    created_by_session_id: Option<Uuid>,
    state: String,
    git_revision: i64,
    head_sha: Option<String>,
    head_subject: Option<String>,
    head_committed_at: Option<DateTime<Utc>>,
    recent_commits_json: serde_json::Value,
    dirty_count: i32,
    untracked_count: i32,
    status_started_at: Option<DateTime<Utc>>,
    status_finished_at: Option<DateTime<Utc>>,
    status_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl WorkspaceViewRow {
    fn into_view(self) -> anyhow::Result<WorkspaceView> {
        let mut recent_commits =
            serde_json::from_value::<Vec<git::Commit>>(self.recent_commits_json.clone())
                .context("deserialize workspace recent commits")?;
        let last_commit = match (
            self.head_sha.as_ref(),
            self.head_subject.as_ref(),
            self.head_committed_at,
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
        let refreshing = match (self.status_started_at, self.status_finished_at) {
            (Some(started), Some(finished)) => started > finished,
            (Some(_), None) => true,
            _ => false,
        };

        let branch = self.branch_name.clone();
        Ok(WorkspaceView {
            id: self.id,
            repo_name: self.repo_name,
            kind: self.kind,
            path: self.path,
            branch_name: self.branch_name,
            base_ref: self.base_ref,
            base_sha: self.base_sha,
            merge_target: self.merge_target,
            created_by_session_id: self.created_by_session_id,
            state: self.state,
            created_at: self.created_at,
            updated_at: self.updated_at,
            git: RepoGitSummary {
                revision: self.git_revision,
                branch,
                uncommitted_count: self.dirty_count,
                untracked_count: self.untracked_count,
                last_commit,
                recent_commits,
                refreshing,
                status_error: self.status_error,
            },
        })
    }
}
