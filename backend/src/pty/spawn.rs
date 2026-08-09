use crate::db::Pool;

use super::{PtyMetadata, PtySessionRepoMetadata, SpawnParams};

pub(super) async fn persist_spawn(
    pool: &Pool,
    meta: &PtyMetadata,
    params: &SpawnParams,
    repositories: &[PtySessionRepoMetadata],
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let persisted = sqlx::query(
        "INSERT INTO pty_sessions \
            (id, repo, working_dir, state, created_at, \
             agent_runtime_agent, agent_runtime_state, agent_runtime_started_at, workspace_id, \
             node_id, node_boot_id, meta_repo_id, meta_repo_name, node_disconnected_at, \
             runtime_end_reason, ended_at, exit_code) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, NULL, NULL, NULL, NULL) \
         ON CONFLICT (id) DO UPDATE SET \
             repo = EXCLUDED.repo, working_dir = EXCLUDED.working_dir, \
             state = EXCLUDED.state, created_at = EXCLUDED.created_at, \
             agent_runtime_agent = EXCLUDED.agent_runtime_agent, \
             agent_runtime_state = EXCLUDED.agent_runtime_state, \
             agent_runtime_started_at = EXCLUDED.agent_runtime_started_at, \
             agent_runtime_ended_at = NULL, agent_runtime_exit_code = NULL, \
             workspace_id = EXCLUDED.workspace_id, node_id = EXCLUDED.node_id, \
             node_boot_id = EXCLUDED.node_boot_id, meta_repo_id = EXCLUDED.meta_repo_id, \
             meta_repo_name = EXCLUDED.meta_repo_name, node_disconnected_at = NULL, \
             runtime_end_reason = NULL, ended_at = NULL, exit_code = NULL \
         WHERE pty_sessions.state <> 'live'",
    )
    .bind(meta.id)
    .bind(&meta.repo)
    .bind(meta.working_dir.to_string_lossy().as_ref())
    .bind(meta.state.as_str())
    .bind(meta.created_at)
    .bind(meta.agent_runtime.agent.as_deref())
    .bind(&meta.agent_runtime.state)
    .bind(meta.agent_runtime.started_at)
    .bind(meta.workspace.as_ref().map(|workspace| workspace.id))
    .bind(params.node_id)
    .bind(params.node_boot_id)
    .bind(meta.meta_repo.as_ref().map(|group| group.id))
    .bind(meta.meta_repo.as_ref().map(|group| group.name.as_str()))
    .execute(&mut *tx)
    .await?;
    if persisted.rows_affected() == 0 {
        anyhow::bail!("PTY session {} already has a live database record", meta.id);
    }

    sqlx::query("DELETE FROM pty_session_repos WHERE pty_session_id = $1")
        .bind(meta.id)
        .execute(&mut *tx)
        .await?;
    for repository in repositories {
        sqlx::query(
            "INSERT INTO pty_session_repos \
                (pty_session_id, repo_name, workspace_id, role, position) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(meta.id)
        .bind(&repository.repo_name)
        .bind(repository.workspace.as_ref().map(|workspace| workspace.id))
        .bind(&repository.role)
        .bind(repository.position)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
