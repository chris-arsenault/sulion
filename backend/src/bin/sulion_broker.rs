use sulion::secret_broker::{app, BrokerConfig, BrokerState};
use sulion::service;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    service::init_tracing();
    let config = BrokerConfig::from_env()?;
    let addr = config.listen;
    let state = BrokerState::from_config(&config).await?;
    service::serve(addr, app(state), "sulion secret broker").await
}
