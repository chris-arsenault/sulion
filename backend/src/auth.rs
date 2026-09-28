use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

use crate::config::AuthConfig;
use crate::AppState;

pub mod revocation;

const JWKS_CACHE_TTL: Duration = Duration::from_secs(60 * 15);
const JWKS_REFRESH_COOLDOWN: Duration = Duration::from_secs(30);

pub struct AuthState {
    config: AuthConfig,
    client: reqwest::Client,
    jwks_cache: RwLock<Option<JwksCache>>,
    last_refresh: Mutex<Option<Instant>>,
}

impl AuthState {
    pub fn new(config: AuthConfig) -> Self {
        let config = AuthConfig {
            issuer_url: config.issuer_url.trim_end_matches('/').to_string(),
            client_id: config.client_id,
        };
        Self {
            config,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(3))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("HTTP client configuration"),
            jwks_cache: RwLock::new(None),
            last_refresh: Mutex::new(None),
        }
    }

    pub async fn validate_bearer(&self, token: &str) -> anyhow::Result<AuthenticatedUser> {
        let header = decode_header(token).context("invalid jwt header")?;
        anyhow::ensure!(header.alg == Algorithm::RS256, "unsupported jwt algorithm");
        let kid = header
            .kid
            .clone()
            .ok_or_else(|| anyhow!("jwt missing kid"))?;
        let key = self.find_key(&kid).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.leeway = 0;
        validation.validate_aud = false; // Access tokens bind the application through client_id.
        validation.set_required_spec_claims(&["exp", "iss", "sub", "iat", "auth_time"]);
        validation.set_issuer(std::slice::from_ref(&self.config.issuer_url));
        let claims = decode::<JwtClaims>(token, &key, &validation)
            .context("jwt validation failed")?
            .claims;

        let matches_client = match claims.token_use.as_deref() {
            Some("access") => claims.client_id.as_deref() == Some(self.config.client_id.as_str()),
            Some(other) => return Err(anyhow!("unsupported token_use {other}")),
            None => return Err(anyhow!("jwt missing token_use")),
        };
        if !matches_client {
            return Err(anyhow!("jwt client mismatch"));
        }
        let now = chrono::Utc::now().timestamp();
        anyhow::ensure!(
            !claims.sub.is_empty()
                && claims.auth_time <= claims.iat
                && claims.iat <= now
                && claims.exp > claims.iat,
            "invalid jwt times or subject"
        );

        Ok(AuthenticatedUser {
            sub: claims.sub,
            username: claims.username.or(claims.cognito_username),
            email: claims.email,
            token_use: claims.token_use.unwrap_or_else(|| "unknown".into()),
            auth_time: claims.auth_time,
            expires_at: claims.exp,
        })
    }

    async fn find_key(&self, kid: &str) -> anyhow::Result<DecodingKey> {
        if let Some(key) = self.find_cached_key(kid).await {
            return Ok(key);
        }
        let mut last = self.last_refresh.lock().await;
        if let Some(key) = self.find_cached_key(kid).await {
            return Ok(key);
        }
        anyhow::ensure!(
            !last.is_some_and(|at| at.elapsed() < JWKS_REFRESH_COOLDOWN),
            "jwks refresh cooling down"
        );
        *last = Some(Instant::now());
        self.refresh_jwks().await?;
        self.find_cached_key(kid)
            .await
            .ok_or_else(|| anyhow!("jwks missing key id"))
    }

    async fn find_cached_key(&self, kid: &str) -> Option<DecodingKey> {
        let cache = self.jwks_cache.read().await;
        let current = cache.as_ref()?;
        if current.loaded_at.elapsed() > JWKS_CACHE_TTL {
            return None;
        }
        current.keys.get(kid).cloned()
    }

    async fn refresh_jwks(&self) -> anyhow::Result<()> {
        let resp = self
            .client
            .get(format!("{}/.well-known/jwks.json", self.config.issuer_url))
            .send()
            .await
            .context("jwks request failed")?
            .error_for_status()
            .context("jwks request failed")?;
        let jwks = resp
            .json::<JwksResponse>()
            .await
            .context("invalid jwks payload")?;
        let mut keys = HashMap::new();
        for key in jwks.keys {
            if key.kty != "RSA" {
                continue;
            }
            let Some(kid) = key.kid else { continue };
            let decoding_key =
                DecodingKey::from_rsa_components(&key.n, &key.e).context("invalid rsa jwk")?;
            keys.insert(kid, decoding_key);
        }
        *self.jwks_cache.write().await = Some(JwksCache {
            loaded_at: Instant::now(),
            keys,
        });
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub sub: String,
    pub username: Option<String>,
    pub email: Option<String>,
    pub token_use: String,
    pub auth_time: i64,
    pub expires_at: i64,
}

impl AuthenticatedUser {
    /// Synthetic principal for explicitly configured development and tests.
    pub fn dev() -> Self {
        Self {
            sub: "dev".to_string(),
            username: Some("dev".to_string()),
            email: None,
            token_use: "dev".to_string(),
            auth_time: 0,
            expires_at: i64::MAX,
        }
    }

    pub fn authority(&self) -> revocation::Authority {
        revocation::Authority {
            sub: self.sub.clone(),
            auth_time: self.auth_time,
            expires_at: self.expires_at,
        }
    }

    pub async fn check_authority(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.expires_at > chrono::Utc::now().timestamp(),
            "browser authority expired"
        );
        if self.token_use != "dev" {
            revocation::check_remote(&self.authority()).await?;
        }
        Ok(())
    }
}

struct JwksCache {
    loaded_at: Instant,
    keys: HashMap<String, DecodingKey>,
}

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<JwkKey>,
}

#[derive(Debug, Deserialize)]
struct JwkKey {
    kid: Option<String>,
    kty: String,
    n: String,
    e: String,
}

#[derive(Debug, Deserialize)]
struct JwtClaims {
    sub: String,
    exp: i64,
    iat: i64,
    auth_time: i64,
    email: Option<String>,
    client_id: Option<String>,
    token_use: Option<String>,
    username: Option<String>,
    #[serde(rename = "cognito:username")]
    cognito_username: Option<String>,
}

pub async fn require_http_auth(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(auth_state) = state.auth.clone() else {
        // Startup permits this only for an explicit test/development bypass.
        req.extensions_mut().insert(AuthenticatedUser::dev());
        return next.run(req).await;
    };

    let token = bearer_from_request(req.headers());
    let Some(token) = token else {
        return unauthorized();
    };

    match auth_state.validate_bearer(token).await {
        Ok(user) => {
            if user.check_authority().await.is_err() {
                return unauthorized();
            }
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(err) => {
            tracing::warn!(error = %err, "authentication failed");
            unauthorized()
        }
    }
}

fn bearer_from_request(headers: &axum::http::HeaderMap) -> Option<&str> {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        if let Ok(value) = value.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                if !token.trim().is_empty() {
                    return Some(token);
                }
            }
        }
    }
    None
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({ "error": "unauthorized" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests;
