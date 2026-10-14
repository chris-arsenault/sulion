//! One durable cutoff per Cognito principal, owned by the broker database.
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::{sync::OnceLock, time::Duration};

#[derive(Debug, thiserror::Error)]
#[error("authority check temporarily unavailable")]
pub(crate) struct AuthorityUnavailable;

pub(crate) const REJECTED_CODE: &str = "BROWSER_AUTHORITY_REJECTED";

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
    let cutoff: Option<i64> = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query_scalar(
            "SELECT revoked_before FROM secret_broker.browser_revocations WHERE user_sub = $1",
        )
        .bind(&authority.sub)
        .fetch_optional(pool),
    )
    .await
    .context(AuthorityUnavailable)?
    .context(AuthorityUnavailable)?;
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
        .ok_or_else(|| anyhow::anyhow!("broker URL required for revocation checks"))
        .context(AuthorityUnavailable)?;
    let token = crate::config::env_optional("SULION_SECRET_BROKER_REGISTRATION_TOKEN")
        .ok_or_else(|| anyhow::anyhow!("broker credential required for revocation checks"))
        .context(AuthorityUnavailable)?;
    let response = client
        .post(format!("{}/v1/auth/check", base.trim_end_matches('/')))
        .bearer_auth(token)
        .json(authority)
        .send()
        .await
        .context(AuthorityUnavailable)?;
    let status = response.status();
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok(());
    }
    // An unmarked 401 can be the broker rejecting its machine credential.
    // Only the broker's explicit authority decision can end a browser login.
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let body: serde_json::Value = response.json().await.context(AuthorityUnavailable)?;
        if body["code"] == REJECTED_CODE {
            anyhow::bail!("browser authority expired or revoked");
        }
    }
    Err(anyhow::anyhow!("authority service returned HTTP {status}").context(AuthorityUnavailable))
}
