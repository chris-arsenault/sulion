//! `/api/repos/:name` lifecycle handlers: rename and delete.

use std::path::Path as StdPath;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::routes::{repo_path, repos_root, validate_repo_name, ApiError, ApiResult};
use crate::{git, AppState};

#[derive(Serialize)]
pub(super) struct RepoView {
    name: String,
    path: String,
}

#[derive(Deserialize)]
pub(super) struct PatchRepoReq {
    name: String,
}

#[derive(Deserialize)]
pub(super) struct DeleteRepoQuery {
    #[serde(default)]
    force: Option<bool>,
}

pub(super) async fn patch_repo(
    State(state): State<Arc<AppState>>,
    Path(old_name): Path<String>,
    Json(req): Json<PatchRepoReq>,
) -> ApiResult<Json<RepoView>> {
    validate_repo_name(&old_name)?;
    let new_name = req.name.trim().to_string();
    validate_repo_name(&new_name)?;

    let root = repos_root(&state)?;
    let old_path = repo_path(&state, &old_name)?;
    if old_name == new_name {
        return Ok(Json(RepoView {
            name: old_name,
            path: old_path.to_string_lossy().into_owned(),
        }));
    }

    ensure_no_live_repo_sessions(&state.pool, &old_name, "rename").await?;
    ensure_no_active_repo_worktrees(&state.pool, &old_name, "rename").await?;
    ensure_repo_name_available(&state.pool, &new_name).await?;

    let new_path = root.join(&new_name);
    if new_path.exists() {
        return Err(ApiError::BadRequest(format!(
            "repo already exists: {}",
            new_path.display()
        )));
    }

    tokio::fs::rename(&old_path, &new_path).await?;
    if let Err(err) =
        rename_repo_records(&state.pool, &old_name, &new_name, &old_path, &new_path).await
    {
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

    Ok(Json(RepoView {
        name: new_name,
        path: new_path.to_string_lossy().into_owned(),
    }))
}

pub(super) async fn delete_repo(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(q): Query<DeleteRepoQuery>,
) -> ApiResult<StatusCode> {
    validate_repo_name(&name)?;
    let path = repo_path(&state, &name)?;

    ensure_no_live_repo_sessions(&state.pool, &name, "delete").await?;
    ensure_no_active_repo_worktrees(&state.pool, &name, "delete").await?;
    ensure_repo_clean_for_delete(&path, q.force.unwrap_or(false)).await?;

    tokio::fs::remove_dir_all(&path).await?;
    mark_repo_deleted_records(&state.pool, &name, &path)
        .await
        .map_err(ApiError::Internal)?;

    Ok(StatusCode::NO_CONTENT)
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

    sqlx::query("DELETE FROM repos WHERE name = $1")
        .bind(new_name)
        .execute(&mut **tx)
        .await?;
    let updated = sqlx::query("UPDATE repos SET name = $2, path = $3 WHERE name = $1")
        .bind(old_name)
        .bind(new_name)
        .bind(new_path)
        .execute(&mut **tx)
        .await?;
    if updated.rows_affected() == 0 {
        sqlx::query("INSERT INTO repos (name, path) VALUES ($1, $2)")
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
    sqlx::query("DELETE FROM repos WHERE name = $1")
        .bind(name)
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
