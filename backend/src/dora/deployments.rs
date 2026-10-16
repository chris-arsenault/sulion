use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use super::{Deployment, OutcomeError, WINDOW_DAYS};
use crate::db::Pool;

const PAGE_SIZE: usize = 25;

#[derive(Debug, Default, Deserialize)]
pub struct DeploymentQuery {
    pub repo: Option<String>,
    pub before: Option<DateTime<Utc>>,
    pub before_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct DeploymentCursor {
    pub deployed_at: DateTime<Utc>,
    pub id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct DeploymentPage {
    pub deployments: Vec<Deployment>,
    pub next_cursor: Option<DeploymentCursor>,
}

pub async fn list_deployments(
    pool: &Pool,
    input: &DeploymentQuery,
) -> Result<DeploymentPage, OutcomeError> {
    if input.before.is_some() != input.before_id.is_some() {
        return Err(OutcomeError::Invalid(
            "Deployment cursor requires both before and before_id.".into(),
        ));
    }
    let now = Utc::now();
    let mut query: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT d.id, d.repo_name, d.revision, d.deployed_at, d.source_url, d.outcome, d.failed_at, d.recovered_at, d.rework \
         FROM dora_deployments d WHERE d.deployed_at >= ",
    );
    query.push_bind(now - chrono::Duration::days(WINDOW_DAYS));
    query.push(" AND d.deployed_at <= ").push_bind(now);
    query.push(" AND EXISTS (SELECT 1 FROM repo_runtime_state r WHERE r.repo_name = d.repo_name AND r.exists = TRUE)");
    if let Some(repo) = &input.repo {
        query.push(" AND d.repo_name = ").push_bind(repo);
    }
    if let (Some(at), Some(id)) = (input.before, input.before_id) {
        query
            .push(" AND (d.deployed_at, d.id) < (")
            .push_bind(at)
            .push(", ")
            .push_bind(id)
            .push(")");
    }
    query
        .push(" ORDER BY d.deployed_at DESC, d.id DESC LIMIT ")
        .push_bind((PAGE_SIZE + 1) as i64);
    let mut deployments: Vec<Deployment> = query.build_query_as().fetch_all(pool).await?;
    let next_cursor = if deployments.len() > PAGE_SIZE {
        deployments.truncate(PAGE_SIZE);
        deployments.last().map(|last| DeploymentCursor {
            deployed_at: last.deployed_at,
            id: last.id,
        })
    } else {
        None
    };
    Ok(DeploymentPage {
        deployments,
        next_cursor,
    })
}
