use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub type Pool = PgPool;

pub async fn connect(db_url: &str) -> anyhow::Result<Pool> {
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .acquire_timeout(Duration::from_secs(10))
        .connect(db_url)
        .await?;
    Ok(pool)
}

pub async fn run_migrations(pool: &Pool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

pub async fn ping(pool: &Pool) -> anyhow::Result<()> {
    let (_ok,): (i32,) = sqlx::query_as("SELECT 1").fetch_one(pool).await?;
    Ok(())
}
