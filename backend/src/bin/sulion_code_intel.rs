use std::net::SocketAddr;

use sulion::code_intel::{app, CodeIntelConfig, CodeIntelState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sulion=debug".into()),
        )
        .init();

    let config = CodeIntelConfig::from_env()?;
    let addr: SocketAddr = config.listen;
    let allowed_roots = config.allowed_roots.clone();
    let state = CodeIntelState::from_config(config).await?;
    tracing::info!(
        listen = %addr,
        allowed_roots = ?allowed_roots,
        "starting sulion code-intel service",
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(state)).await?;
    Ok(())
}
