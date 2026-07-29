use sulion::code_intel::{app, CodeIntelConfig, CodeIntelState};
use sulion::service;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    service::init_tracing();
    let config = CodeIntelConfig::from_env()?;
    let addr = config.listen;
    // Logged before serving because a wrong root set is the usual reason this
    // service answers nothing useful, and it is not visible from any request.
    tracing::info!(allowed_roots = ?config.allowed_roots, "code-intel allowed roots");
    let state = CodeIntelState::from_config(config).await?;
    service::serve(addr, app(state), "sulion code-intel service").await
}
