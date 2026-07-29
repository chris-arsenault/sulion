use sulion::container_runner::{app, RunnerConfig, RunnerState};
use sulion::service;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    service::init_tracing();
    let config = RunnerConfig::from_env()?;
    let addr = config.listen;
    // Which docker it drives and which roots it will mount are the two things
    // that decide whether this runner can do anything, and neither is visible
    // from a request.
    tracing::info!(
        docker_bin = %config.docker_bin.display(),
        allowed_roots = ?config.allowed_roots,
        "container runner configuration",
    );
    let state = RunnerState::new(config);
    service::serve(addr, app(state), "sulion container runner").await
}
