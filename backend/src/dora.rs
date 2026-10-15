//! Production delivery evidence. Metrics reads only Postgres; node collection
//! lives in `repo_ci::dora`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Pool;

pub const WINDOW_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize)]
pub struct DoraMetrics {
    pub window_days: i64,
    pub repos: Vec<RepoDoraMetrics>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RepoDoraMetrics {
    pub repo: String,
    pub checked_at: Option<DateTime<Utc>>,
    pub scanned_since: Option<DateTime<Utc>>,
    pub collection_error: Option<String>,
    pub deployments: i64,
    pub deployments_per_day: Option<f64>,
    pub change_lead_time_hours_p50: Option<f64>,
    pub lead_time_samples: i64,
    pub classified_outcomes: i64,
    pub failed_deployments: i64,
    pub change_failure_rate: Option<f64>,
    pub recovery_time_hours_p50: Option<f64>,
    pub recovered_deployments: i64,
    pub open_failures: i64,
    pub classified_rework: i64,
    pub rework_rate: Option<f64>,
    #[sqlx(skip)]
    pub recent_deployments: Vec<Deployment>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Deployment {
    pub id: Uuid,
    pub repo_name: String,
    pub revision: String,
    pub deployed_at: DateTime<Utc>,
    pub source_url: String,
    pub outcome: String,
    pub failed_at: Option<DateTime<Utc>>,
    pub recovered_at: Option<DateTime<Utc>>,
    pub rework: Option<bool>,
}

/// An atomic replacement of an operator's production classification.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentOutcome {
    pub outcome: String,
    pub failed_at: Option<DateTime<Utc>>,
    pub recovered_at: Option<DateTime<Utc>>,
    pub rework: Option<bool>,
}

impl DeploymentOutcome {
    pub fn validate(&self, deployed_at: DateTime<Utc>, now: DateTime<Utc>) -> Result<(), String> {
        if !matches!(self.outcome.as_str(), "unknown" | "healthy" | "failed") {
            return Err("Outcome must be unknown, healthy or failed.".into());
        }
        if (self.outcome == "failed") != self.failed_at.is_some() {
            return Err(
                "A failed outcome requires a failure time; other outcomes cannot have one.".into(),
            );
        }
        if self
            .failed_at
            .is_some_and(|at| at < deployed_at || at > now)
        {
            return Err("Failure time must be between deployment and now.".into());
        }
        if let Some(recovered) = self.recovered_at {
            if self.failed_at.is_none_or(|failed| recovered < failed) || recovered > now {
                return Err("Recovery time must be between failure and now.".into());
            }
        }
        Ok(())
    }
}

pub async fn metrics(pool: &Pool) -> anyhow::Result<DoraMetrics> {
    let now = Utc::now();
    let since = now - chrono::Duration::days(WINDOW_DAYS);
    let mut repos: Vec<RepoDoraMetrics> = sqlx::query_as(
        "WITH deployments AS ( \
           SELECT * FROM dora_deployments WHERE deployed_at >= $1 AND deployed_at <= $2 \
         ), counts AS ( \
           SELECT repo_name, COUNT(*) AS deployments, \
             COUNT(*) FILTER (WHERE outcome <> 'unknown') AS classified_outcomes, \
             COUNT(*) FILTER (WHERE outcome = 'failed') AS failed_deployments, \
             COUNT(rework) AS classified_rework, COUNT(*) FILTER (WHERE rework) AS rework_count, \
             COUNT(recovered_at) AS recovered_deployments, \
             COUNT(*) FILTER (WHERE outcome = 'failed' AND recovered_at IS NULL) AS open_failures, \
             percentile_cont(0.5) WITHIN GROUP (ORDER BY EXTRACT(EPOCH FROM (recovered_at - failed_at)) / 3600.0) \
               FILTER (WHERE recovered_at IS NOT NULL) AS recovery_time_hours_p50 \
           FROM deployments GROUP BY repo_name \
         ), changes AS ( \
           SELECT c.repo_name, COUNT(*) AS lead_time_samples, \
             percentile_cont(0.5) WITHIN GROUP (ORDER BY EXTRACT(EPOCH FROM (d.deployed_at - c.committed_at)) / 3600.0) \
               AS change_lead_time_hours_p50 \
           FROM dora_commit_deployments c JOIN deployments d ON d.id = c.deployment_id \
           WHERE NOT c.baseline AND c.committed_at <= d.deployed_at GROUP BY c.repo_name \
         ) \
         SELECT r.repo_name AS repo, r.dora_checked_at AS checked_at, r.dora_scanned_since AS scanned_since, \
           r.dora_error AS collection_error, COALESCE(n.deployments, 0) AS deployments, \
           CASE WHEN r.dora_checked_at IS NOT NULL THEN COALESCE(n.deployments, 0)::float8 / $3 END AS deployments_per_day, \
           c.change_lead_time_hours_p50, COALESCE(c.lead_time_samples, 0) AS lead_time_samples, \
           COALESCE(n.classified_outcomes, 0) AS classified_outcomes, \
           COALESCE(n.failed_deployments, 0) AS failed_deployments, \
           CASE WHEN n.deployments > 0 AND n.classified_outcomes = n.deployments \
             THEN n.failed_deployments::float8 / n.deployments END AS change_failure_rate, \
           n.recovery_time_hours_p50, COALESCE(n.recovered_deployments, 0) AS recovered_deployments, \
           COALESCE(n.open_failures, 0) AS open_failures, COALESCE(n.classified_rework, 0) AS classified_rework, \
           CASE WHEN n.deployments > 0 AND n.classified_rework = n.deployments \
             THEN n.rework_count::float8 / n.deployments END AS rework_rate \
         FROM repo_runtime_state r LEFT JOIN counts n ON n.repo_name = r.repo_name \
         LEFT JOIN changes c ON c.repo_name = r.repo_name \
         WHERE r.exists = TRUE ORDER BY r.repo_name",
    ).bind(since).bind(now).bind(WINDOW_DAYS as f64).fetch_all(pool).await?;
    let deployments: Vec<Deployment> = sqlx::query_as(
        "SELECT id, repo_name, revision, deployed_at, source_url, outcome, failed_at, recovered_at, rework \
         FROM dora_deployments WHERE deployed_at >= $1 AND deployed_at <= $2 ORDER BY deployed_at DESC, id",
    ).bind(since).bind(now).fetch_all(pool).await?;
    let mut by_repo: std::collections::HashMap<String, Vec<Deployment>> =
        std::collections::HashMap::new();
    for deployment in deployments {
        by_repo
            .entry(deployment.repo_name.clone())
            .or_default()
            .push(deployment);
    }
    for repo in &mut repos {
        repo.recent_deployments = by_repo.remove(&repo.repo).unwrap_or_default();
    }
    Ok(DoraMetrics {
        window_days: WINDOW_DAYS,
        repos,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum OutcomeError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Storage(#[from] sqlx::Error),
}

pub async fn update_outcome(
    pool: &Pool,
    id: Uuid,
    input: &DeploymentOutcome,
) -> Result<Option<Deployment>, OutcomeError> {
    let deployed_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT deployed_at FROM dora_deployments WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some(deployed_at) = deployed_at else {
        return Ok(None);
    };
    input
        .validate(deployed_at, Utc::now())
        .map_err(OutcomeError::Invalid)?;
    Ok(sqlx::query_as(
        "UPDATE dora_deployments SET outcome = $2, failed_at = $3, recovered_at = $4, rework = $5 \
         WHERE id = $1 RETURNING id, repo_name, revision, deployed_at, source_url, outcome, failed_at, recovered_at, rework",
    ).bind(id).bind(&input.outcome).bind(input.failed_at).bind(input.recovered_at).bind(input.rework).fetch_optional(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_requires_ordered_real_failure_and_recovery_times() {
        let deployed = Utc::now() - chrono::Duration::hours(3);
        let now = Utc::now();
        let mut input = DeploymentOutcome {
            outcome: "failed".into(),
            failed_at: None,
            recovered_at: None,
            rework: None,
        };
        assert!(input.validate(deployed, now).is_err());
        input.failed_at = Some(deployed + chrono::Duration::hours(1));
        assert!(input.validate(deployed, now).is_ok());
        input.recovered_at = Some(deployed);
        assert!(input.validate(deployed, now).is_err());
        input.recovered_at = Some(now + chrono::Duration::hours(1));
        assert!(input.validate(deployed, now).is_err());
        input.recovered_at = Some(now);
        assert!(input.validate(deployed, now).is_ok());
        input.outcome = "healthy".into();
        assert!(input.validate(deployed, now).is_err());
    }
}
