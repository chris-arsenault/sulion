use shuttlecraft::ingester::{Ingester, IngesterConfig};
use shuttlecraft::{app, config::Config, db, AppState};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env()?;
    tracing::info!(listen = %cfg.listen, "shuttlecraft starting");

    let pool = db::connect(&cfg.db_url).await?;
    db::run_migrations(&pool).await?;
    tracing::info!("migrations applied");

    // Any row still marked 'live' belongs to a shell the prior backend
    // was supervising — the process died with its parent and nobody
    // captured exit. Roll those to 'orphaned'.
    let orphaned = shuttlecraft::pty::reconcile_orphans_on_startup(&pool).await?;
    if orphaned > 0 {
        tracing::info!(count = orphaned, "reconciled orphaned PTY sessions");
    }

    // Background ingester — the sole reader of the JSONL transcripts.
    let ingester_pool = pool.clone();
    let ingester_cfg = IngesterConfig::new(cfg.claude_projects_dir.clone());
    tokio::spawn(async move {
        Ingester::new().run(ingester_pool, ingester_cfg).await;
    });
    tracing::info!(
        projects = %cfg.claude_projects_dir.display(),
        "ingester started",
    );

    // SessionStart-hook correlation socket.
    let correlate_pool = pool.clone();
    let correlate_sock = cfg.correlate_sock_path.clone();
    tokio::spawn(async move {
        if let Err(err) = shuttlecraft::correlate::run(correlate_pool, correlate_sock).await {
            tracing::error!(%err, "correlate socket exited");
        }
    });

    let state = AppState::new(pool, cfg.repos_root.clone());
    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    axum::serve(listener, app(state)).await?;
    Ok(())
}
