//! Production delivery evidence. Metrics reads only Postgres; node collection
//! lives in `repo_ci::dora`.

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Pool;

mod deployments;
pub use deployments::{list_deployments, DeploymentCursor, DeploymentPage, DeploymentQuery};

pub const WINDOW_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize)]
pub struct DoraMetrics {
    pub window_days: i64,
    pub summary: RepoDoraMetrics,
    pub repos: Vec<RepoDoraMetrics>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RepoDoraMetrics {
    pub repo: String,
    pub checked_at: Option<DateTime<Utc>>,
    pub scanned_since: Option<DateTime<Utc>>,
    pub collection_error: Option<String>,
    pub repository_count: i64,
    pub collected_repository_count: i64,
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
    /// Empty during rolling upgrades; deployment records use the paged reader.
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
    let rows: Vec<RepoDoraMetrics> = sqlx::query_as(include_str!("dora/metrics.sql"))
        .bind(since)
        .bind(now)
        .bind(WINDOW_DAYS as f64)
        .fetch_all(pool)
        .await?;
    let mut rows = rows.into_iter();
    let summary = rows
        .next()
        .context("DORA query omitted portfolio summary")?;
    Ok(DoraMetrics {
        window_days: WINDOW_DAYS,
        summary,
        repos: rows.collect(),
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
