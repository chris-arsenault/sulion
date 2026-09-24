use std::sync::Arc;
use std::time::Duration;

use sulion::ingest::{Ingester, IngesterConfig};

const RESTART_BACKOFF: Duration = Duration::from_secs(1);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sulion=debug".into()),
        )
        .init();

    let config = sulion::config::Config::from_env()?;
    let pool = sulion::db::connect_and_wait_for_migrations(&config.db_url, "ingester").await?;
    let ingester = Arc::new(Ingester::new());
    let ingester_config = IngesterConfig::new(config.claude_projects_dir)
        .with_codex_sessions_dir(config.codex_sessions_dir);
    tracing::info!("starting node-local transcript ingester");

    loop {
        let task = {
            let ingester = ingester.clone();
            let pool = pool.clone();
            let config = ingester_config.clone();
            tokio::spawn(async move {
                ingester.run(pool, config).await;
            })
        };
        match task.await {
            Ok(()) => tracing::error!("ingester exited unexpectedly"),
            Err(error) => tracing::error!(%error, "ingester task failed"),
        }
        tokio::time::sleep(RESTART_BACKOFF).await;
    }
}
