use sulion::retrieval::{app, RetrievalConfig, RetrievalState};
use sulion::service;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    service::init_tracing();
    let config = RetrievalConfig::from_env()?;
    let addr = config.listen;
    let state = RetrievalState::from_config(config).await?;
    service::serve(addr, app(state), "sulion retrieval service").await
}
