//! The devenv server binary: owns PTY masters and dials the node over the
//! unix socket on the shared run volume. Reconnecting is this process's job —
//! when the node is recreated, the shells here keep running and this loop
//! dials back in.

use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sulion=debug".into()),
        )
        .init();

    let socket_path = std::env::var("SULION_DEVENV_SOCK")
        .unwrap_or_else(|_| "/run/sulion/devenv.sock".to_string());
    // Child mode: the node spawned this process and owns its lifetime, so a
    // closed connection means the parent is gone on purpose — exit instead of
    // redialing. Container mode leaves this unset and redials forever.
    let exit_on_disconnect = std::env::var("SULION_DEVENV_EXIT_ON_DISCONNECT")
        .map(|value| value == "1")
        .unwrap_or(false);

    let server = Arc::new(sulion::devenv::server::DevenvServer::new());
    tracing::info!(%socket_path, exit_on_disconnect, "devenv server starting");

    let mut backoff = Duration::from_millis(200);
    let mut failing_since: Option<std::time::Instant> = None;
    loop {
        match tokio::net::UnixStream::connect(&socket_path).await {
            Ok(stream) => {
                tracing::info!(%socket_path, "devenv connected to node");
                backoff = Duration::from_millis(200);
                failing_since = None;
                server.clone().serve(stream).await;
                let live = server.live_session_ids().await.len();
                tracing::warn!(live_sessions = live, "devenv lost the node connection");
                if exit_on_disconnect {
                    return Ok(());
                }
            }
            Err(error) => {
                tracing::debug!(%error, %socket_path, "devenv dial failed; retrying");
                if exit_on_disconnect {
                    // The parent that spawned us also owns the listener; if
                    // it never answers, it is gone and so is our reason to
                    // exist.
                    let since = *failing_since.get_or_insert_with(std::time::Instant::now);
                    if since.elapsed() > Duration::from_secs(30) {
                        anyhow::bail!("node socket never became dialable: {error}");
                    }
                }
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}
