use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::db::Pool;
use crate::git::{self, DiffStat, GitStatus};
use crate::repo_state::RepoGitSummary;

const WORKSPACE_SCAN_INTERVAL: Duration = Duration::from_secs(30);
const WORKSPACE_STATUS_CADENCE_SECS: i32 = 30;
const WORKSPACE_STATUS_ERROR_CADENCE_SECS: i32 = 90;

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceRecord {
    pub id: Uuid,
    pub repo_name: String,
    pub kind: String,
    pub path: PathBuf,
    pub branch_name: Option<String>,
    pub base_ref: Option<String>,
    pub base_sha: Option<String>,
    pub merge_target: Option<String>,
    pub created_by_session_id: Option<Uuid>,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceView {
    pub id: Uuid,
    pub repo_name: String,
    pub kind: String,
    pub path: String,
    pub branch_name: Option<String>,
    pub base_ref: Option<String>,
    pub base_sha: Option<String>,
    pub merge_target: Option<String>,
    pub created_by_session_id: Option<Uuid>,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub git: RepoGitSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceDirtyPaths {
    pub workspace_id: Uuid,
    pub git_revision: i64,
    pub dirty_by_path: HashMap<String, String>,
    pub diff_stats_by_path: HashMap<String, DiffStat>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DeleteWorkspaceOptions {
    pub force: bool,
    pub delete_branch: bool,
}

#[derive(Clone)]
pub struct WorkspaceManager {
    pool: Pool,
    repos_root: PathBuf,
    workspaces_root: PathBuf,
}

impl WorkspaceManager {
    pub fn new(pool: Pool, repos_root: PathBuf, workspaces_root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            pool,
            repos_root,
            workspaces_root,
        })
    }

    pub async fn run(self: Arc<Self>) {
        let mut last_scan = std::time::Instant::now() - WORKSPACE_SCAN_INTERVAL;
        loop {
            if last_scan.elapsed() >= WORKSPACE_SCAN_INTERVAL {
                if let Err(err) = self.sync_main_workspaces_once().await {
                    tracing::warn!(%err, "workspace sync failed");
                }
                last_scan = std::time::Instant::now();
            }
            if let Err(err) = self.reconcile_due_once(2).await {
                tracing::warn!(%err, "workspace reconcile failed");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    pub async fn sync_main_workspaces_once(&self) -> anyhow::Result<()> {
        let repos = discover_repo_dirs(&self.repos_root).await?;
        for (name, path) in repos {
            self.ensure_main_workspace(&name, &path).await?;
        }
        Ok(())
    }

    pub async fn ensure_main_workspace(
        &self,
        repo_name: &str,
        repo_path: &Path,
    ) -> anyhow::Result<WorkspaceRecord> {
        validate_repo_name(repo_name)?;
        let branch = current_branch(repo_path).await.unwrap_or(None);
        let head = rev_parse(repo_path, "HEAD").await.ok();
        let existing: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM workspaces \
              WHERE repo_name = $1 AND kind = 'main' AND state <> 'deleted'",
        )
        .bind(repo_name)
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("load main workspace for {repo_name}"))?;

        match existing {
            Some((id,)) => {
                sqlx::query(
                    "UPDATE workspaces \
                        SET path = $2, branch_name = $3, base_ref = $3, base_sha = $4, \
                            merge_target = $3, state = 'active', next_status_at = NOW(), \
                            updated_at = NOW() \
                      WHERE id = $1",
                )
                .bind(id)
                .bind(repo_path.to_string_lossy().as_ref())
                .bind(branch.as_deref())
                .bind(head.as_deref())
                .execute(&self.pool)
                .await
                .with_context(|| format!("update main workspace for {repo_name}"))?;
                self.load_workspace(id).await
            }
            None => {
                let id = Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO workspaces \
                        (id, repo_name, kind, path, branch_name, base_ref, base_sha, merge_target, \
                         state, next_status_at, created_at, updated_at) \
                     VALUES ($1, $2, 'main', $3, $4, $4, $5, $4, 'active', NOW(), NOW(), NOW())",
                )
                .bind(id)
                .bind(repo_name)
                .bind(repo_path.to_string_lossy().as_ref())
                .bind(branch.as_deref())
                .bind(head.as_deref())
                .execute(&self.pool)
                .await
                .with_context(|| format!("insert main workspace for {repo_name}"))?;
                self.load_workspace(id).await
            }
        }
    }

    pub async fn create_worktree_workspace(
        &self,
        repo_name: &str,
    ) -> anyhow::Result<WorkspaceRecord> {
        validate_repo_name(repo_name)?;
        let repo_path = self.repos_root.join(repo_name);
        if !repo_path.is_dir() {
            anyhow::bail!("repo does not exist: {}", repo_path.display());
        }
        if !git::is_git_repo(&repo_path) {
            anyhow::bail!(
                "isolated workspace requires a git repo: {}",
                repo_path.display()
            );
        }

        let base_ref = current_branch(&repo_path)
            .await?
            .unwrap_or_else(|| "HEAD".to_string());
        let base_sha = rev_parse(&repo_path, "HEAD")
            .await
            .context("isolated workspace requires an existing HEAD commit")?;
        let id = Uuid::new_v4();
        let short = id.simple().to_string();
        let short = &short[..12];
        let branch_name = format!("sulion/{}/{}", branch_component(repo_name), short);
        let workspace_path = self.workspaces_root.join(repo_name).join(id.to_string());
        if workspace_path.exists() {
            anyhow::bail!(
                "workspace path already exists: {}",
                workspace_path.display()
            );
        }
        if let Some(parent) = workspace_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create workspace parent {}", parent.display()))?;
        }

        run_git_checked(
            &repo_path,
            &[
                "worktree",
                "add",
                "-b",
                &branch_name,
                workspace_path.to_string_lossy().as_ref(),
                &base_ref,
            ],
        )
        .await
        .with_context(|| format!("create git worktree {}", workspace_path.display()))?;

        sqlx::query(
            "INSERT INTO workspaces \
                (id, repo_name, kind, path, branch_name, base_ref, base_sha, merge_target, \
                 state, next_status_at, created_at, updated_at) \
             VALUES ($1, $2, 'worktree', $3, $4, $5, $6, $5, 'active', NOW(), NOW(), NOW())",
        )
        .bind(id)
        .bind(repo_name)
        .bind(workspace_path.to_string_lossy().as_ref())
        .bind(&branch_name)
        .bind(&base_ref)
        .bind(&base_sha)
        .execute(&self.pool)
        .await
        .with_context(|| format!("insert worktree workspace for {repo_name}"))?;

        self.load_workspace(id).await
    }

    pub async fn load_workspace(&self, id: Uuid) -> anyhow::Result<WorkspaceRecord> {
        load_workspace(&self.pool, id).await
    }

    pub async fn bind_created_session(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE workspaces \
                SET created_by_session_id = COALESCE(created_by_session_id, $2), updated_at = NOW() \
              WHERE id = $1",
        )
        .bind(workspace_id)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .with_context(|| format!("bind session {session_id} to workspace {workspace_id}"))?;
        Ok(())
    }

    pub async fn request_refresh(&self, id: Uuid) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE workspaces \
                SET next_status_at = NOW(), updated_at = NOW() \
              WHERE id = $1 AND state = 'active'",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .with_context(|| format!("request workspace refresh for {id}"))?;
        Ok(())
    }

    pub async fn delete_workspace(
        &self,
        id: Uuid,
        options: DeleteWorkspaceOptions,
    ) -> anyhow::Result<()> {
        let workspace = self.load_workspace(id).await?;
        if workspace.kind == "main" {
            anyhow::bail!("main workspaces cannot be deleted");
        }
        if workspace.kind != "worktree" {
            anyhow::bail!("unsupported workspace kind: {}", workspace.kind);
        }

        let active_sessions: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) \
               FROM pty_sessions \
              WHERE workspace_id = $1 AND state IN ('live', 'orphaned')",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("count active sessions for workspace {id}"))?;
        if active_sessions.0 > 0 {
            anyhow::bail!(
                "workspace is still bound to {} live or orphaned session{}",
                active_sessions.0,
                if active_sessions.0 == 1 { "" } else { "s" }
            );
        }

        let repo_path = self.repos_root.join(&workspace.repo_name);
        if !repo_path.is_dir() {
            anyhow::bail!("repo does not exist: {}", repo_path.display());
        }

        if workspace.path.is_dir() {
            let status = git::read_status(workspace.path.clone()).await?;
            if status.uncommitted_count > 0 && !options.force {
                anyhow::bail!(
                    "workspace has {} uncommitted change{}; retry with force to discard it",
                    status.uncommitted_count,
                    if status.uncommitted_count == 1 {
                        ""
                    } else {
                        "s"
                    }
                );
            }
        }

        let branch_exists = match workspace.branch_name.as_deref() {
            Some(branch) => git_branch_exists(&repo_path, branch).await?,
            None => false,
        };
        if options.delete_branch && branch_exists && !options.force {
            match (
                workspace.base_sha.as_deref(),
                workspace.branch_name.as_deref(),
            ) {
                (Some(base_sha), Some(branch)) => {
                    let unique =
                        rev_list_count(&repo_path, &format!("{base_sha}..{branch}")).await?;
                    if unique > 0 {
                        anyhow::bail!(
                            "workspace branch has {} commit{} not in its base; retry with force to delete it",
                            unique,
                            if unique == 1 { "" } else { "s" }
                        );
                    }
                }
                _ => {
                    anyhow::bail!(
                        "workspace branch history cannot be checked; retry with force to delete it"
                    );
                }
            }
        }

        let worktree_registered = git_worktree_registered(&repo_path, &workspace.path).await?;
        if workspace.path.is_dir() || worktree_registered {
            let mut args = vec!["worktree", "remove"];
            if options.force {
                args.push("--force");
            }
            let workspace_path = workspace.path.to_string_lossy().into_owned();
            args.push(&workspace_path);
            run_git_checked(&repo_path, &args)
                .await
                .with_context(|| format!("remove git worktree {}", workspace.path.display()))?;
        }

        if options.delete_branch {
            if let Some(branch) = workspace.branch_name.as_deref() {
                if git_branch_exists(&repo_path, branch).await? {
                    run_git_checked(&repo_path, &["branch", "-D", branch])
                        .await
                        .with_context(|| format!("delete workspace branch {branch}"))?;
                }
            }
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .context("begin workspace delete tx")?;
        sqlx::query("DELETE FROM workspace_dirty_paths WHERE workspace_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("clear dirty paths for workspace {id}"))?;
        sqlx::query("UPDATE pty_sessions SET workspace_id = NULL WHERE workspace_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("unbind sessions from workspace {id}"))?;
        sqlx::query(
            "UPDATE workspaces \
                SET state = 'deleted', status_error = NULL, updated_at = NOW() \
              WHERE id = $1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("mark workspace {id} deleted"))?;
        tx.commit()
            .await
            .with_context(|| format!("commit workspace delete for {id}"))?;
        Ok(())
    }

    pub async fn reconcile_due_once(&self, limit: i64) -> anyhow::Result<usize> {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, path \
               FROM workspaces \
              WHERE state = 'active' AND next_status_at <= NOW() \
              ORDER BY next_status_at ASC, created_at ASC \
              LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("load due workspace state rows")?;

        let mut count = 0;
        for (id, path) in rows {
            self.reconcile_workspace(id, PathBuf::from(path)).await;
            count += 1;
        }
        Ok(count)
    }

    async fn reconcile_workspace(&self, id: Uuid, path: PathBuf) {
        if let Err(err) = self.reconcile_workspace_inner(id, &path).await {
            tracing::warn!(%id, path = %path.display(), %err, "workspace status reconcile failed");
            let _ = sqlx::query(
                "UPDATE workspaces \
                    SET status_error = $2, \
                        status_finished_at = NOW(), \
                        next_status_at = NOW() + make_interval(secs => $3), \
                        updated_at = NOW() \
                  WHERE id = $1",
            )
            .bind(id)
            .bind(err.to_string())
            .bind(WORKSPACE_STATUS_ERROR_CADENCE_SECS)
            .execute(&self.pool)
            .await;
        }
    }

    async fn reconcile_workspace_inner(&self, id: Uuid, path: &Path) -> anyhow::Result<()> {
        if !path.is_dir() {
            sqlx::query(
                "UPDATE workspaces \
                    SET state = 'missing', status_error = $2, status_finished_at = NOW(), updated_at = NOW() \
                  WHERE id = $1",
            )
            .bind(id)
            .bind(format!("workspace path missing: {}", path.display()))
            .execute(&self.pool)
            .await?;
            return Ok(());
        }

        sqlx::query(
            "UPDATE workspaces \
                SET status_started_at = NOW(), status_error = NULL, updated_at = NOW() \
              WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .with_context(|| format!("mark workspace reconcile started for {id}"))?;

        let status = git::read_status(path.to_path_buf()).await?;
        let fingerprint = status_fingerprint(&status);
        let recent_commits_json =
            serde_json::to_value(&status.recent_commits).context("serialize recent commits")?;
        let last = status.last_commit.as_ref();
        let last_committed_at: Option<DateTime<Utc>> = last.and_then(|commit| {
            DateTime::parse_from_rfc3339(&commit.committed_at)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        });

        let mut tx = self
            .pool
            .begin()
            .await
            .context("begin workspace reconcile tx")?;
        sqlx::query("DELETE FROM workspace_dirty_paths WHERE workspace_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("clear dirty paths for workspace {id}"))?;
        for (path, code) in &status.dirty_by_path {
            let diff = status.diff_stats_by_path.get(path);
            sqlx::query(
                "INSERT INTO workspace_dirty_paths (workspace_id, path, status, additions, deletions) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(path)
            .bind(code)
            .bind(diff.map(|d| d.additions as i32))
            .bind(diff.map(|d| d.deletions as i32))
            .execute(&mut *tx)
            .await
            .with_context(|| format!("insert dirty path {path} for workspace {id}"))?;
        }

        sqlx::query(
            "UPDATE workspaces \
                SET git_revision = CASE \
                      WHEN dirty_fingerprint IS DISTINCT FROM $2 THEN git_revision + 1 \
                      ELSE git_revision \
                    END, \
                    branch_name = $3, \
                    head_sha = $4, \
                    head_subject = $5, \
                    head_committed_at = $6, \
                    recent_commits_json = $7, \
                    dirty_count = $8, \
                    untracked_count = $9, \
                    dirty_fingerprint = $2, \
                    status_finished_at = NOW(), \
                    next_status_at = NOW() + make_interval(secs => $10), \
                    status_error = NULL, \
                    updated_at = NOW() \
              WHERE id = $1",
        )
        .bind(id)
        .bind(&fingerprint)
        .bind(&status.branch)
        .bind(last.map(|c| c.sha.as_str()))
        .bind(last.map(|c| c.subject.as_str()))
        .bind(last_committed_at)
        .bind(recent_commits_json)
        .bind(status.uncommitted_count as i32)
        .bind(status.untracked_count as i32)
        .bind(WORKSPACE_STATUS_CADENCE_SECS)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("update workspace state for {id}"))?;
        tx.commit()
            .await
            .with_context(|| format!("commit workspace reconcile for {id}"))?;
        Ok(())
    }
}

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

pub async fn run_cli(args: &[OsString]) -> anyhow::Result<i32> {
    match args.first().and_then(|arg| arg.to_str()) {
        Some("status") => {
            print_workspace_status();
            Ok(0)
        }
        _ => {
            eprintln!("usage: sulion workspace status");
            Ok(2)
        }
    }
}

fn print_workspace_status() {
    let keys = [
        "SULION_REPO_NAME",
        "SULION_WORKSPACE_ID",
        "SULION_WORKSPACE_KIND",
        "SULION_WORKSPACE_PATH",
        "SULION_CANONICAL_REPO",
        "SULION_BRANCH",
        "SULION_BASE_REF",
        "SULION_BASE_SHA",
        "SULION_MERGE_TARGET",
    ];
    if std::env::var("SULION_WORKSPACE_ID").is_err() {
        println!("No Sulion workspace metadata is present in this shell.");
        return;
    }
    println!("Sulion workspace");
    for key in keys {
        let value = std::env::var(key).unwrap_or_else(|_| String::new());
        println!("{}={}", key, value);
    }
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

async fn discover_repo_dirs(root: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut entries = tokio::fs::read_dir(root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let meta = match entry.metadata().await {
            Ok(meta) => meta,
            Err(err) => {
                tracing::warn!(path = %entry.path().display(), %err, "workspace repo stat failed");
                continue;
            }
        };
        if !meta.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        out.push((name, entry.path()));
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(out)
}

fn validate_repo_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        anyhow::bail!("invalid repo name");
    }
    Ok(())
}

fn branch_component(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    if out.is_empty() {
        "repo".to_string()
    } else {
        out
    }
}

async fn current_branch(repo_path: &Path) -> anyhow::Result<Option<String>> {
    let out = run_git_capture(repo_path, &["branch", "--show-current"]).await?;
    let branch = out.trim().to_string();
    Ok(if branch.is_empty() {
        None
    } else {
        Some(branch)
    })
}

async fn rev_parse(repo_path: &Path, rev: &str) -> anyhow::Result<String> {
    let out = run_git_capture(repo_path, &["rev-parse", rev]).await?;
    let value = out.trim().to_string();
    if value.is_empty() {
        anyhow::bail!("git rev-parse {rev} returned empty output");
    }
    Ok(value)
}

async fn git_branch_exists(repo_path: &Path, branch: &str) -> anyhow::Result<bool> {
    let repo_path = repo_path.to_path_buf();
    let branch = branch.to_string();
    tokio::task::spawn_blocking(move || {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo_path)
            .args(["show-ref", "--verify", "--quiet"])
            .arg(format!("refs/heads/{branch}"))
            .status()?;
        Ok(status.success())
    })
    .await?
}

async fn rev_list_count(repo_path: &Path, range: &str) -> anyhow::Result<u64> {
    let out = run_git_capture(repo_path, &["rev-list", "--count", range]).await?;
    out.trim()
        .parse::<u64>()
        .with_context(|| format!("parse git rev-list count for {range}"))
}

async fn git_worktree_registered(repo_path: &Path, workspace_path: &Path) -> anyhow::Result<bool> {
    let out = run_git_capture(repo_path, &["worktree", "list", "--porcelain"]).await?;
    Ok(out.lines().any(|line| {
        line.strip_prefix("worktree ")
            .is_some_and(|path| Path::new(path) == workspace_path)
    }))
}

async fn run_git_capture(repo_path: &Path, args: &[&str]) -> anyhow::Result<String> {
    let repo_path = repo_path.to_path_buf();
    let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo_path)
            .args(&args)
            .output()?;
        if !out.status.success() {
            anyhow::bail!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    })
    .await?
}

async fn run_git_checked(repo_path: &Path, args: &[&str]) -> anyhow::Result<()> {
    run_git_capture(repo_path, args).await.map(|_| ())
}

fn status_fingerprint(status: &GitStatus) -> String {
    let mut parts = Vec::new();
    parts.push(format!("branch={}", status.branch.as_deref().unwrap_or("")));
    parts.push(format!(
        "head={}",
        status
            .last_commit
            .as_ref()
            .map(|commit| commit.sha.as_str())
            .unwrap_or("")
    ));
    let mut dirty = status.dirty_by_path.iter().collect::<Vec<_>>();
    dirty.sort_by(|left, right| left.0.cmp(right.0));
    for (path, code) in dirty {
        let diff = status.diff_stats_by_path.get(path);
        let additions = diff.map(|d| d.additions).unwrap_or(0);
        let deletions = diff.map(|d| d.deletions).unwrap_or(0);
        parts.push(format!("{path}:{code}:{additions}:{deletions}"));
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_component_keeps_git_safe_chars() {
        assert_eq!(branch_component("the-canonry_game.1"), "the-canonry_game.1");
    }

    #[test]
    fn branch_component_replaces_unsafe_chars() {
        assert_eq!(branch_component("bad/repo name"), "bad-repo-name");
    }
}
