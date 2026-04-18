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

    let state = AppState::new(pool);
    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    axum::serve(listener, app(state)).await?;
    Ok(())
}
