use super::*;

/// Rebuild the derived usage tables from canonical event payloads. The usage
/// tables are locked before the event snapshot is read: an ingester transaction
/// that has inserted a newer event will then apply its usage update after this
/// transaction commits, so the rebuild cannot erase concurrent usage.
pub(crate) async fn rebuild_usage_projection(
    pool: &crate::db::Pool,
    from_version: i32,
) -> anyhow::Result<u64> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "LOCK TABLE agent_session_usage, agent_usage_daily, agent_model_usage_daily, agent_usage_responses \
         IN ACCESS EXCLUSIVE MODE",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM agent_usage_responses")
        .execute(&mut *tx)
        .await?;
    if from_version < 3 {
        rebuild_legacy(&mut tx).await?;
    }
    let mut sessions = records::rebuild(&mut tx).await?;
    if from_version < 3 {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_session_usage")
            .fetch_one(&mut *tx)
            .await?;
        sessions = total as u64;
    }
    tx.commit().await?;
    Ok(sessions)
}

async fn rebuild_legacy(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM agent_model_usage_daily WHERE session_uuid IN (SELECT session_uuid FROM events)")
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "DELETE FROM agent_usage_daily WHERE session_uuid IN (SELECT session_uuid FROM events)",
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM agent_session_usage WHERE session_uuid IN (SELECT session_uuid FROM events)",
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query(include_str!("codex_sessions.sql"))
        .execute(&mut **tx)
        .await?
        .rows_affected();

    sqlx::query(include_str!("claude_sessions.sql"))
        .execute(&mut **tx)
        .await?
        .rows_affected();

    sqlx::query(include_str!("daily_snapshots.sql"))
        .execute(&mut **tx)
        .await?;

    sqlx::query(include_str!("model_daily.sql"))
        .execute(&mut **tx)
        .await?;

    Ok(())
}
