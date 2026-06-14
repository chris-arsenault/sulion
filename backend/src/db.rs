use anyhow::Context;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
use std::collections::HashSet;
use std::time::Duration;
use tokio::time::sleep;

pub type Pool = PgPool;

const MIGRATION_WAIT_INTERVAL: Duration = Duration::from_secs(2);
const MIGRATION_LOG_EVERY_ATTEMPTS: u64 = 15;

pub async fn connect(db_url: &str) -> anyhow::Result<Pool> {
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .acquire_timeout(Duration::from_secs(10))
        .connect(db_url)
        .await?;
    Ok(pool)
}

pub async fn connect_and_wait_for_migrations(
    db_url: &str,
    service_name: &str,
) -> anyhow::Result<Pool> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match connect(db_url).await {
            Ok(pool) => {
                wait_for_migrations(&pool, service_name).await?;
                return Ok(pool);
            }
            Err(err) => {
                if should_log_wait(attempts) {
                    tracing::warn!(
                        service = service_name,
                        %err,
                        "waiting for database connection before service startup"
                    );
                }
                sleep(MIGRATION_WAIT_INTERVAL).await;
            }
        }
    }
}

pub async fn run_migrations(pool: &Pool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

pub async fn wait_for_migrations(pool: &Pool, service_name: &str) -> anyhow::Result<()> {
    let expected_versions = embedded_migration_versions();
    if expected_versions.is_empty() {
        anyhow::bail!("embedded migration set is empty");
    }

    let mut attempts = 0;
    loop {
        attempts += 1;
        match migration_readiness(pool, &expected_versions).await {
            Ok(readiness) if readiness.is_ready() => {
                if attempts > 1 {
                    tracing::info!(
                        service = service_name,
                        applied = readiness.applied_count,
                        expected = readiness.expected_count,
                        "backend database migrations are ready"
                    );
                }
                return Ok(());
            }
            Ok(readiness) => {
                if should_log_wait(attempts) {
                    tracing::info!(
                        service = service_name,
                        migration_table_exists = readiness.table_exists,
                        applied = readiness.applied_count,
                        expected = readiness.expected_count,
                        failed = readiness.failed_count,
                        latest_expected_version = readiness.latest_expected_version,
                        latest_success_version = ?readiness.latest_success_version,
                        "waiting for backend database migrations"
                    );
                }
            }
            Err(err) => {
                if should_log_wait(attempts) {
                    tracing::warn!(
                        service = service_name,
                        %err,
                        "waiting for backend database migrations"
                    );
                }
            }
        }
        sleep(MIGRATION_WAIT_INTERVAL).await;
    }
}

pub async fn ping(pool: &Pool) -> anyhow::Result<()> {
    let (_ok,): (i32,) = sqlx::query_as("SELECT 1").fetch_one(pool).await?;
    Ok(())
}

fn embedded_migration_versions() -> Vec<i64> {
    sqlx::migrate!("./migrations")
        .migrations
        .iter()
        .map(|migration| migration.version)
        .collect()
}

async fn migration_readiness(
    pool: &Pool,
    expected_versions: &[i64],
) -> anyhow::Result<MigrationReadiness> {
    let table_name: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations')::text")
            .fetch_one(pool)
            .await
            .context("check migration table")?;
    let latest_expected_version = expected_versions.iter().copied().max().unwrap_or_default();
    let Some(_) = table_name else {
        return Ok(MigrationReadiness {
            table_exists: false,
            expected_count: expected_versions.len(),
            applied_count: 0,
            failed_count: 0,
            latest_expected_version,
            latest_success_version: None,
        });
    };

    let rows = sqlx::query("SELECT version, success FROM _sqlx_migrations WHERE version = ANY($1)")
        .bind(expected_versions)
        .fetch_all(pool)
        .await
        .context("read migration status")?;

    let mut successful_versions = HashSet::new();
    let mut failed_count = 0;
    for row in rows {
        let version: i64 = row.get("version");
        let success: bool = row.get("success");
        if success {
            successful_versions.insert(version);
        } else {
            failed_count += 1;
        }
    }
    let latest_success_version = successful_versions.iter().copied().max();

    Ok(MigrationReadiness {
        table_exists: true,
        expected_count: expected_versions.len(),
        applied_count: successful_versions.len(),
        failed_count,
        latest_expected_version,
        latest_success_version,
    })
}

fn should_log_wait(attempts: u64) -> bool {
    attempts == 1 || attempts.is_multiple_of(MIGRATION_LOG_EVERY_ATTEMPTS)
}

struct MigrationReadiness {
    table_exists: bool,
    expected_count: usize,
    applied_count: usize,
    failed_count: usize,
    latest_expected_version: i64,
    latest_success_version: Option<i64>,
}

impl MigrationReadiness {
    fn is_ready(&self) -> bool {
        self.table_exists && self.applied_count == self.expected_count && self.failed_count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_migration_versions_are_present_and_ordered() {
        let versions = embedded_migration_versions();
        assert!(!versions.is_empty());
        assert!(versions.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
