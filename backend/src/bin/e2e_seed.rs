#[tokio::main]
async fn main() -> anyhow::Result<()> {
    sulion::e2e::seed_from_env().await
}
