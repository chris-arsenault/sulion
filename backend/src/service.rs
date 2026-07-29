//! The bit every service binary repeats: set up tracing, bind, serve.
//!
//! The four HTTP services — broker, retrieval, code intelligence, runner — were
//! the same twenty-line program with the type names swapped. What actually
//! differs between them is which config they read and which state they build,
//! so that is all their `main` should contain.

use std::net::SocketAddr;

use axum::Router;

/// Structured logs, with `RUST_LOG` overriding the default filter.
///
/// `json` matches the control plane and the node, so everything a deployment
/// emits parses the same way.
pub fn init_tracing() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sulion=debug".into()),
        )
        .init();
}

/// Binds and serves until the process is stopped. `what` names the service in
/// the startup line, which is the only log an operator sees before traffic.
pub async fn serve(addr: SocketAddr, app: Router, what: &str) -> anyhow::Result<()> {
    tracing::info!(listen = %addr, "starting {what}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
