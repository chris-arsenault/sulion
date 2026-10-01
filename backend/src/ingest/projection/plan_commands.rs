//! Enrich retained operation rows without replaying turns or changing their IDs.
use serde_json::Value;
use uuid::Uuid;

use crate::db::Pool;
use crate::ingest::canonical::OperationCategory;

#[derive(sqlx::FromRow)]
struct Operation {
    turn_id: i64,
    operation_ord: i32,
    input: Value,
    operation_type: Option<String>,
    operation_category: Option<String>,
}

pub(crate) async fn backfill(pool: &Pool) -> anyhow::Result<usize> {
    let sessions: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT o.session_uuid FROM timeline_operations o \
         JOIN claude_sessions s USING (session_uuid) WHERE s.purged_at IS NULL \
         AND (o.input ? 'command' OR o.input ? 'cmd' OR o.input ? 'operations')",
    )
    .fetch_all(pool)
    .await?;
    let mut affected = 0;
    for session in sessions {
        affected += usize::from(backfill_session(pool, session).await?);
    }
    Ok(affected)
}

async fn backfill_session(pool: &Pool, session: Uuid) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    // Share the incremental writer's lock; append and enrichment cannot race.
    let locked: Option<i64> = sqlx::query_scalar(
        "SELECT revision FROM timeline_session_state WHERE session_uuid = $1 FOR UPDATE",
    )
    .bind(session)
    .fetch_optional(&mut *tx)
    .await?;
    let purged: bool = sqlx::query_scalar(
        "SELECT purged_at IS NOT NULL FROM claude_sessions WHERE session_uuid = $1",
    )
    .bind(session)
    .fetch_one(&mut *tx)
    .await?;
    if locked.is_none() || purged {
        return Ok(false);
    }
    let mut after = (-1_i64, -1_i32);
    let mut changed = false;
    loop {
        let rows: Vec<Operation> = sqlx::query_as(
            "SELECT turn_id, operation_ord, input, operation_type, operation_category \
             FROM timeline_operations WHERE session_uuid = $1 \
             AND (turn_id, operation_ord) > ($2, $3) \
             AND (input ? 'command' OR input ? 'cmd' OR input ? 'operations') \
             ORDER BY turn_id, operation_ord LIMIT 256",
        )
        .bind(session)
        .bind(after.0)
        .bind(after.1)
        .fetch_all(&mut *tx)
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after = (row.turn_id, row.operation_ord);
            let mut input = row.input.clone();
            let mut kind = row.operation_type.clone();
            let mut category = row
                .operation_category
                .as_deref()
                .and_then(OperationCategory::parse);
            if !crate::ingest::timeline::plan_commands::project(
                &mut input,
                &mut kind,
                &mut category,
            ) || (input == row.input
                && kind == row.operation_type
                && category.map(|category| category.as_str()) == row.operation_category.as_deref())
            {
                continue;
            }
            sqlx::query(
                "UPDATE timeline_operations SET input = $4, operation_type = $5, \
                 operation_category = $6 WHERE session_uuid = $1 AND turn_id = $2 \
                 AND operation_ord = $3",
            )
            .bind(session)
            .bind(row.turn_id)
            .bind(row.operation_ord)
            .bind(input)
            .bind(kind)
            .bind(category.map(|category| category.as_str()))
            .execute(&mut *tx)
            .await?;
            changed = true;
        }
    }
    if changed {
        sqlx::query(
            "UPDATE timeline_session_state SET revision = revision + 1, \
             generation = gen_random_uuid() WHERE session_uuid = $1",
        )
        .bind(session)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(changed)
}
