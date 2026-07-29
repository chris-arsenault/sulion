//! Renaming and deleting a repo: the checks that decide whether it is allowed,
//! the directory move, and the records that have to follow it.
//!
//! This is domain logic, not HTTP. It lives outside `api` because the node
//! runtime performs these operations — it owns the repos directory — and a node
//! reaching into the HTTP layer for them inverted the dependency between the
//! two. `api/repo_lifecycle_routes.rs` is the thin handler over this.
//!
//! The record updates reach into tables owned by ingest and retrieval. That is
//! deliberate: a repo rename has to carry its whole history with it. It is also
//! the fragile part — a schema change in either module lands here.

use std::path::Path as StdPath;

use crate::git;

/// Why a lifecycle operation was refused. Each variant is a distinct outcome
/// for the caller: a repo that is not there, a refusal it can act on, and
/// everything else.
#[derive(Debug, thiserror::Error)]
pub enum RepoLifecycleError {
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Internal(#[from] anyhow::Error),
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

type ApiResult<T> = Result<T, RepoLifecycleError>;
use RepoLifecycleError as ApiError;

fn validate_repo_name(name: &str) -> ApiResult<()> {
    if !crate::workspace::is_valid_repo_name(name) {
        return Err(ApiError::BadRequest("invalid repo name".into()));
    }
    Ok(())
}

pub async fn rename_repo_runtime(
    pool: &crate::db::Pool,
    root: &StdPath,
    old_name: &str,
    new_name: &str,
) -> ApiResult<std::path::PathBuf> {
    validate_repo_name(old_name)?;
    validate_repo_name(new_name)?;
    let old_path = root.join(old_name);
    if !old_path.is_dir() {
        return Err(ApiError::NotFound);
    }
    if old_name == new_name {
        return Ok(old_path);
    }
    ensure_no_live_repo_sessions(pool, old_name, "rename").await?;
    ensure_no_active_repo_worktrees(pool, old_name, "rename").await?;
    ensure_repo_name_available(pool, new_name).await?;
    let new_path = root.join(new_name);
    if new_path.exists() {
        return Err(ApiError::BadRequest(format!(
            "repo already exists: {}",
            new_path.display()
        )));
    }
    tokio::fs::rename(&old_path, &new_path).await?;
    if let Err(err) = rename_repo_records(pool, old_name, new_name, &old_path, &new_path).await {
        if let Err(rollback_err) = tokio::fs::rename(&new_path, &old_path).await {
            tracing::error!(
                repo = %old_name,
                new_repo = %new_name,
                %rollback_err,
                "failed to roll back repo directory rename after database error",
            );
        }
        return Err(ApiError::Internal(err));
    }
    Ok(new_path)
}

pub async fn delete_repo_runtime(
    pool: &crate::db::Pool,
    root: &StdPath,
    name: &str,
    force: bool,
) -> ApiResult<()> {
    validate_repo_name(name)?;
    let path = root.join(name);
    if !path.is_dir() {
        return Err(ApiError::NotFound);
    }
    ensure_no_live_repo_sessions(pool, name, "delete").await?;
    ensure_no_active_repo_worktrees(pool, name, "delete").await?;
    ensure_repo_clean_for_delete(&path, force).await?;
    tokio::fs::remove_dir_all(&path).await?;
    mark_repo_deleted_records(pool, name, &path)
        .await
        .map_err(ApiError::Internal)
}

async fn ensure_no_live_repo_sessions(
    pool: &crate::db::Pool,
    name: &str,
    action: &str,
) -> ApiResult<()> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT \
           FROM pty_sessions \
          WHERE repo = $1 AND state IN ('live', 'orphaned')",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .map_err(ApiError::Db)?;

    if count > 0 {
        return Err(ApiError::BadRequest(format!(
            "cannot {action} repo with {count} live or orphaned session(s)"
        )));
    }
    Ok(())
}

async fn ensure_no_active_repo_worktrees(
    pool: &crate::db::Pool,
    name: &str,
    action: &str,
) -> ApiResult<()> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT \
           FROM workspaces \
          WHERE repo_name = $1 AND kind = 'worktree' AND state <> 'deleted'",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .map_err(ApiError::Db)?;

    if count > 0 {
        return Err(ApiError::BadRequest(format!(
            "cannot {action} repo with {count} active isolated workspace(s); delete them first"
        )));
    }
    Ok(())
}

async fn ensure_repo_name_available(pool: &crate::db::Pool, name: &str) -> ApiResult<()> {
    let in_use: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM pty_sessions WHERE repo = $1 AND state <> 'deleted' \
             UNION ALL \
             SELECT 1 FROM workspaces WHERE repo_name = $1 AND state <> 'deleted' \
             UNION ALL \
             SELECT 1 FROM repo_runtime_state WHERE repo_name = $1 AND exists = TRUE \
         )",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .map_err(ApiError::Db)?;

    if in_use {
        return Err(ApiError::BadRequest(format!(
            "repo name is already in use: {name}"
        )));
    }
    Ok(())
}

async fn ensure_repo_clean_for_delete(path: &StdPath, force: bool) -> ApiResult<()> {
    if force {
        return Ok(());
    }
    let status = git::read_status(path.to_path_buf())
        .await
        .map_err(ApiError::Internal)?;
    if status.uncommitted_count > 0 {
        return Err(ApiError::BadRequest(format!(
            "repo has {} uncommitted change(s); retry with force=true to delete it",
            status.uncommitted_count
        )));
    }
    Ok(())
}

async fn rename_repo_records(
    pool: &crate::db::Pool,
    old_name: &str,
    new_name: &str,
    old_path: &StdPath,
    new_path: &StdPath,
) -> anyhow::Result<()> {
    let old_path = old_path.to_string_lossy().into_owned();
    let new_path = new_path.to_string_lossy().into_owned();
    let mut tx = pool.begin().await?;

    rename_repo_runtime_records(&mut tx, old_name, new_name, &new_path).await?;
    rename_repo_session_workspace_records(&mut tx, old_name, new_name, &old_path, &new_path)
        .await?;
    rename_repo_index_records(&mut tx, old_name, new_name, &old_path, &new_path).await?;

    tx.commit().await?;
    Ok(())
}

async fn rename_repo_runtime_records(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    old_name: &str,
    new_name: &str,
    new_path: &str,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM repo_dirty_paths WHERE repo_name IN ($1, $2)")
        .bind(old_name)
        .bind(new_name)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM repo_runtime_state WHERE repo_name = $1")
        .bind(new_name)
        .execute(&mut **tx)
        .await?;
    let updated = sqlx::query(
        "UPDATE repo_runtime_state \
            SET repo_name = $2, path = $3, exists = TRUE, next_status_at = NOW(), updated_at = NOW() \
          WHERE repo_name = $1",
    )
    .bind(old_name)
    .bind(new_name)
    .bind(new_path)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 0 {
        sqlx::query(
            "INSERT INTO repo_runtime_state (repo_name, path, exists, next_status_at, updated_at) \
             VALUES ($1, $2, TRUE, NOW(), NOW())",
        )
        .bind(new_name)
        .bind(new_path)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

async fn rename_repo_session_workspace_records(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    old_name: &str,
    new_name: &str,
    old_path: &str,
    new_path: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE pty_sessions \
            SET repo = $2, \
                working_dir = CASE \
                    WHEN working_dir = $3 THEN $4 \
                    WHEN substr(working_dir, 1, length($3) + 1) = $3 || '/' \
                        THEN $4 || substr(working_dir, length($3) + 1) \
                    ELSE working_dir \
                END \
          WHERE repo = $1",
    )
    .bind(old_name)
    .bind(new_name)
    .bind(old_path)
    .bind(new_path)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "UPDATE agent_session_metadata \
            SET cwd = CASE \
                    WHEN cwd = $1 THEN $2 \
                    WHEN substr(cwd, 1, length($1) + 1) = $1 || '/' \
                        THEN $2 || substr(cwd, length($1) + 1) \
                    ELSE cwd \
                END, \
                updated_at = NOW() \
          WHERE cwd = $1 OR substr(cwd, 1, length($1) + 1) = $1 || '/'",
    )
    .bind(old_path)
    .bind(new_path)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "UPDATE workspaces \
            SET repo_name = $2, \
                path = CASE \
                    WHEN path = $3 THEN $4 \
                    WHEN substr(path, 1, length($3) + 1) = $3 || '/' \
                        THEN $4 || substr(path, length($3) + 1) \
                    ELSE path \
                END, \
                updated_at = NOW() \
          WHERE repo_name = $1 AND kind = 'main' AND state <> 'deleted'",
    )
    .bind(old_name)
    .bind(new_name)
    .bind(old_path)
    .bind(new_path)
    .execute(&mut **tx)
    .await?;

    sqlx::query("UPDATE timeline_file_touches SET repo_name = $2 WHERE repo_name = $1")
        .bind(old_name)
        .bind(new_name)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE plans \
            SET repo_name = $2, revision = revision + 1, updated_at = NOW() \
          WHERE repo_name = $1",
    )
    .bind(old_name)
    .bind(new_name)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn rename_repo_index_records(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    old_name: &str,
    new_name: &str,
    old_path: &str,
    new_path: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE retrieval_embeddings SET repo_name = $2 WHERE repo_name = $1")
        .bind(old_name)
        .bind(new_name)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE retrieval_embedding_sources SET repo_name = $2 WHERE repo_name = $1")
        .bind(old_name)
        .bind(new_name)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE retrieval_embedding_backfills SET scope_repo = $2 WHERE scope_repo = $1")
        .bind(old_name)
        .bind(new_name)
        .execute(&mut **tx)
        .await?;

    sqlx::query(
        "UPDATE code_roots \
            SET deleted_at = NOW(), updated_at = NOW() \
          WHERE root_kind = 'repo' AND path = $1 AND deleted_at IS NULL",
    )
    .bind(new_path)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE code_roots \
            SET name = $2, path = $4, repo_name = $2, updated_at = NOW() \
          WHERE root_kind = 'repo' \
            AND deleted_at IS NULL \
            AND (name = $1 OR path = $3 OR repo_name = $1)",
    )
    .bind(old_name)
    .bind(new_name)
    .bind(old_path)
    .bind(new_path)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

async fn mark_repo_deleted_records(
    pool: &crate::db::Pool,
    name: &str,
    path: &StdPath,
) -> anyhow::Result<()> {
    let path = path.to_string_lossy().into_owned();
    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM repo_dirty_paths WHERE repo_name = $1")
        .bind(name)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE repo_runtime_state \
            SET exists = FALSE, path = $2, next_status_at = NOW(), updated_at = NOW() \
          WHERE repo_name = $1",
    )
    .bind(name)
    .bind(&path)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE workspaces \
            SET state = 'deleted', updated_at = NOW() \
          WHERE repo_name = $1 AND kind = 'main' AND state <> 'deleted'",
    )
    .bind(name)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE code_roots \
            SET deleted_at = NOW(), updated_at = NOW() \
          WHERE root_kind = 'repo' \
            AND deleted_at IS NULL \
            AND (name = $1 OR path = $2 OR repo_name = $1)",
    )
    .bind(name)
    .bind(&path)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}
