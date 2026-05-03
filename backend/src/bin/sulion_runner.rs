use std::net::SocketAddr;

use sulion::container_runner::{app, RunnerConfig, RunnerState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sulion=debug".into()),
        )
        .init();

    let config = RunnerConfig::from_env()?;
    let addr: SocketAddr = config.listen;
    let docker_bin = config.docker_bin.clone();
    let allowed_roots = config.allowed_roots.clone();
    let state = RunnerState::new(config);
    tracing::info!(
        listen = %addr,
        docker_bin = %docker_bin.display(),
        allowed_roots = ?allowed_roots,
        "starting sulion container runner",
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(state)).await?;
    Ok(())
}
