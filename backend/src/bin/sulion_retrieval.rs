use std::net::SocketAddr;

use sulion::retrieval::{app, RetrievalConfig, RetrievalState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sulion=debug".into()),
        )
        .init();

    let config = RetrievalConfig::from_env()?;
    let addr: SocketAddr = config.listen;
    let state = RetrievalState::from_config(config).await?;
    tracing::info!(listen = %addr, "starting sulion retrieval service");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(state)).await?;
    Ok(())
}
