use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::db::Pool;
use crate::git::{self, DiffStat, GitStatus};

const REPO_SCAN_INTERVAL: Duration = Duration::from_secs(30);
const REPO_STATUS_CADENCE_SECS: i32 = 30;
const REPO_STATUS_ERROR_CADENCE_SECS: i32 = 90;

#[derive(Debug, Clone, Serialize)]
pub struct RepoGitSummary {
    pub revision: i64,
    pub branch: Option<String>,
    pub uncommitted_count: i32,
    pub untracked_count: i32,
    pub last_commit: Option<git::Commit>,
    pub recent_commits: Vec<git::Commit>,
    pub refreshing: bool,
    pub status_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoDirtyPaths {
    pub repo: String,
    pub git_revision: i64,
    pub dirty_by_path: HashMap<String, String>,
    pub diff_stats_by_path: HashMap<String, DiffStat>,
}

#[derive(Clone)]
pub struct RepoStateManager {
    pool: Pool,
    repos_root: PathBuf,
}

impl RepoStateManager {
    pub fn new(pool: Pool, repos_root: PathBuf) -> Arc<Self> {
        Arc::new(Self { pool, repos_root })
    }

    pub async fn run(self: Arc<Self>) {
        let mut last_scan = std::time::Instant::now() - REPO_SCAN_INTERVAL;
        loop {
            if last_scan.elapsed() >= REPO_SCAN_INTERVAL {
                if let Err(err) = self.sync_repos_once().await {
                    tracing::warn!(%err, "repo state sync failed");
                }
                last_scan = std::time::Instant::now();
            }
            if let Err(err) = self.reconcile_due_once(1).await {
                tracing::warn!(%err, "repo state reconcile failed");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    pub async fn sync_repos_once(&self) -> anyhow::Result<()> {
        let repos = discover_repo_dirs(&self.repos_root).await?;
        let mut tx = self.pool.begin().await.context("begin repo state sync")?;
        sqlx::query("UPDATE repo_runtime_state SET exists = FALSE, updated_at = NOW()")
            .execute(&mut *tx)
            .await
            .context("mark repos absent")?;
        for (name, path) in repos {
            sqlx::query(
                "INSERT INTO repo_runtime_state (repo_name, path, exists, next_status_at, updated_at) \
                 VALUES ($1, $2, TRUE, NOW(), NOW()) \
                 ON CONFLICT (repo_name) DO UPDATE SET \
                   path = EXCLUDED.path, \
                   exists = TRUE, \
                   next_status_at = LEAST(repo_runtime_state.next_status_at, NOW()), \
                   updated_at = NOW()",
            )
            .bind(&name)
            .bind(path.to_string_lossy().as_ref())
            .execute(&mut *tx)
            .await
            .with_context(|| format!("upsert repo state for {name}"))?;
        }
        tx.commit().await.context("commit repo state sync")?;
        Ok(())
    }

    pub async fn upsert_repo(&self, name: &str, path: &Path) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO repo_runtime_state (repo_name, path, exists, next_status_at, updated_at) \
             VALUES ($1, $2, TRUE, NOW(), NOW()) \
             ON CONFLICT (repo_name) DO UPDATE SET \
               path = EXCLUDED.path, \
               exists = TRUE, \
               next_status_at = NOW(), \
               updated_at = NOW()",
        )
        .bind(name)
        .bind(path.to_string_lossy().as_ref())
        .execute(&self.pool)
        .await
        .with_context(|| format!("upsert repo state for {name}"))?;
        Ok(())
    }

    pub async fn request_refresh(&self, name: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE repo_runtime_state \
                SET next_status_at = NOW(), updated_at = NOW() \
              WHERE repo_name = $1 AND exists = TRUE",
        )
        .bind(name)
        .execute(&self.pool)
        .await
        .with_context(|| format!("request repo refresh for {name}"))?;
        Ok(())
    }

    pub async fn reconcile_due_once(&self, limit: i64) -> anyhow::Result<usize> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT repo_name, path \
               FROM repo_runtime_state \
              WHERE exists = TRUE AND next_status_at <= NOW() \
              ORDER BY next_status_at ASC, repo_name ASC \
              LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("load due repo state rows")?;

        let mut count = 0;
        for (name, path) in rows {
            self.reconcile_repo(name, PathBuf::from(path)).await;
            count += 1;
        }
        Ok(count)
    }

    async fn reconcile_repo(&self, name: String, path: PathBuf) {
        if let Err(err) = self.reconcile_repo_inner(&name, &path).await {
            tracing::warn!(repo = %name, path = %path.display(), %err, "repo status reconcile failed");
            let _ = sqlx::query(
                "UPDATE repo_runtime_state \
                    SET status_error = $2, \
                        status_finished_at = NOW(), \
                        next_status_at = NOW() + make_interval(secs => $3), \
                        updated_at = NOW() \
                  WHERE repo_name = $1",
            )
            .bind(&name)
            .bind(err.to_string())
            .bind(REPO_STATUS_ERROR_CADENCE_SECS)
            .execute(&self.pool)
            .await;
        }
    }

    async fn reconcile_repo_inner(&self, name: &str, path: &Path) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE repo_runtime_state \
                SET status_started_at = NOW(), status_error = NULL, updated_at = NOW() \
              WHERE repo_name = $1",
        )
        .bind(name)
        .execute(&self.pool)
        .await
        .with_context(|| format!("mark repo reconcile started for {name}"))?;

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

        let mut tx = self.pool.begin().await.context("begin repo reconcile tx")?;
        sqlx::query("DELETE FROM repo_dirty_paths WHERE repo_name = $1")
            .bind(name)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("clear dirty paths for {name}"))?;
        for (path, code) in &status.dirty_by_path {
            let diff = status.diff_stats_by_path.get(path);
            sqlx::query(
                "INSERT INTO repo_dirty_paths (repo_name, path, status, additions, deletions) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(name)
            .bind(path)
            .bind(code)
            .bind(diff.map(|d| d.additions as i32))
            .bind(diff.map(|d| d.deletions as i32))
            .execute(&mut *tx)
            .await
            .with_context(|| format!("insert dirty path {path} for {name}"))?;
        }

        sqlx::query(
            "UPDATE repo_runtime_state \
                SET git_revision = CASE \
                      WHEN dirty_fingerprint IS DISTINCT FROM $2 THEN git_revision + 1 \
                      ELSE git_revision \
                    END, \
                    branch = $3, \
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
              WHERE repo_name = $1",
        )
        .bind(name)
        .bind(&fingerprint)
        .bind(&status.branch)
        .bind(last.map(|c| c.sha.as_str()))
        .bind(last.map(|c| c.subject.as_str()))
        .bind(last_committed_at)
        .bind(recent_commits_json)
        .bind(status.uncommitted_count as i32)
        .bind(status.untracked_count as i32)
        .bind(REPO_STATUS_CADENCE_SECS)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("update repo state for {name}"))?;
        tx.commit()
            .await
            .with_context(|| format!("commit repo reconcile for {name}"))?;
        Ok(())
    }
}

pub async fn load_dirty_paths(pool: &Pool, repo: &str) -> anyhow::Result<RepoDirtyPaths> {
    let (git_revision,): (i64,) = sqlx::query_as(
        "SELECT git_revision FROM repo_runtime_state WHERE repo_name = $1 AND exists = TRUE",
    )
    .bind(repo)
    .fetch_one(pool)
    .await
    .with_context(|| format!("load git revision for {repo}"))?;

    let rows: Vec<(String, String, Option<i32>, Option<i32>)> = sqlx::query_as(
        "SELECT path, status, additions, deletions \
           FROM repo_dirty_paths \
          WHERE repo_name = $1 \
          ORDER BY path ASC",
    )
    .bind(repo)
    .fetch_all(pool)
    .await
    .with_context(|| format!("load dirty paths for {repo}"))?;

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

    Ok(RepoDirtyPaths {
        repo: repo.to_string(),
        git_revision,
        dirty_by_path,
        diff_stats_by_path,
    })
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
                tracing::warn!(path = %entry.path().display(), %err, "repo dir stat failed");
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
