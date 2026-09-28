//! One durable cutoff per Cognito principal, owned by the broker database.
use serde::{Deserialize, Serialize};
use std::{sync::OnceLock, time::Duration};

#[derive(Serialize, Deserialize)]
pub struct Authority {
    pub sub: String,
    pub auth_time: i64,
    pub expires_at: i64,
}

pub async fn allowed(pool: &crate::db::Pool, authority: &Authority) -> anyhow::Result<bool> {
    if authority.expires_at <= chrono::Utc::now().timestamp() {
        return Ok(false);
    }
    let cutoff: Option<i64> = sqlx::query_scalar(
        "SELECT revoked_before FROM secret_broker.browser_revocations WHERE user_sub = $1",
    )
    .bind(&authority.sub)
    .fetch_optional(pool)
    .await?;
    Ok(cutoff.is_none_or(|at| authority.auth_time > at))
}

pub async fn revoke(pool: &crate::db::Pool, sub: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!sub.trim().is_empty(), "principal is required");
    sqlx::query("INSERT INTO secret_broker.browser_revocations (user_sub, revoked_before) VALUES ($1, floor(extract(epoch FROM clock_timestamp()))::BIGINT) ON CONFLICT (user_sub) DO UPDATE SET revoked_before = GREATEST(secret_broker.browser_revocations.revoked_before, EXCLUDED.revoked_before)")
        .bind(sub).execute(pool).await?;
    Ok(())
}

pub async fn check_remote(authority: &Authority) -> anyhow::Result<()> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("revocation HTTP client")
    });
    let base = crate::config::env_optional("SULION_SECRET_BROKER_URL")
        .ok_or_else(|| anyhow::anyhow!("broker URL required for revocation checks"))?;
    let token = crate::config::env_optional("SULION_SECRET_BROKER_REGISTRATION_TOKEN")
        .ok_or_else(|| anyhow::anyhow!("broker credential required for revocation checks"))?;
    let response = client
        .post(format!("{}/v1/auth/check", base.trim_end_matches('/')))
        .bearer_auth(token)
        .json(authority)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("revocation check unavailable"))?;
    anyhow::ensure!(
        response.status() == reqwest::StatusCode::NO_CONTENT,
        "browser authority rejected"
    );
    Ok(())
}
