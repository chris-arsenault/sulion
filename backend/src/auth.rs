use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use axum::extract::{Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::config::AuthConfig;
use crate::AppState;

const JWKS_CACHE_TTL: Duration = Duration::from_secs(60 * 15);

pub struct AuthState {
    config: AuthConfig,
    client: reqwest::Client,
    jwks_cache: RwLock<Option<JwksCache>>,
}

impl AuthState {
    pub fn new(config: AuthConfig) -> Self {
        let config = AuthConfig {
            issuer_url: config.issuer_url.trim_end_matches('/').to_string(),
            client_id: config.client_id,
        };
        Self {
            config,
            client: reqwest::Client::new(),
            jwks_cache: RwLock::new(None),
        }
    }

    pub async fn validate_bearer(&self, token: &str) -> anyhow::Result<AuthenticatedUser> {
        let header = decode_header(token).context("invalid jwt header")?;
        let kid = header
            .kid
            .clone()
            .ok_or_else(|| anyhow!("jwt missing kid"))?;
        let key = self.find_key(&kid).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(std::slice::from_ref(&self.config.issuer_url));
        let claims = decode::<JwtClaims>(token, &key, &validation)
            .context("jwt validation failed")?
            .claims;

        let matches_client = match claims.token_use.as_deref() {
            Some("access") => claims.client_id.as_deref() == Some(self.config.client_id.as_str()),
            Some("id") => claims.aud.as_deref() == Some(self.config.client_id.as_str()),
            Some(other) => return Err(anyhow!("unsupported token_use {other}")),
            None => return Err(anyhow!("jwt missing token_use")),
        };
        if !matches_client {
            return Err(anyhow!("jwt client mismatch"));
        }

        Ok(AuthenticatedUser {
            sub: claims.sub,
            username: claims.username.or(claims.cognito_username),
            email: claims.email,
            token_use: claims.token_use.unwrap_or_else(|| "unknown".into()),
        })
    }

    async fn find_key(&self, kid: &str) -> anyhow::Result<DecodingKey> {
        if let Some(key) = self.find_cached_key(kid).await {
            return Ok(key);
        }
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
}

impl AuthenticatedUser {
    /// Synthetic principal used when JWT auth is disabled (local dev, most
    /// unit/integration tests). Handlers that record "who did this" — like
    /// device-pairing approval — get a stable identity instead of failing to
    /// extract one.
    pub fn dev() -> Self {
        Self {
            sub: "dev".to_string(),
            username: Some("dev".to_string()),
            email: None,
            token_use: "dev".to_string(),
        }
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
    aud: Option<String>,
    email: Option<String>,
    client_id: Option<String>,
    token_use: Option<String>,
    username: Option<String>,
    #[serde(rename = "cognito:username")]
    cognito_username: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AccessTokenQuery {
    pub access_token: Option<String>,
}

pub async fn require_http_auth(
    State(state): State<Arc<AppState>>,
    query: Query<AccessTokenQuery>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(auth_state) = state.auth.clone() else {
        // Auth disabled: still surface a principal so handlers that read the
        // authenticated user (e.g. device-pairing approval) work in dev/tests.
        req.extensions_mut().insert(AuthenticatedUser::dev());
        return next.run(req).await;
    };

    let token = bearer_from_request(req.headers(), query.access_token.as_deref());
    let Some(token) = token else {
        return unauthorized();
    };

    match auth_state.validate_bearer(token).await {
        Ok(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(err) => {
            tracing::warn!(error = %err, "authentication failed");
            unauthorized()
        }
    }
}

fn bearer_from_request<'a>(
    headers: &'a axum::http::HeaderMap,
    query_token: Option<&'a str>,
) -> Option<&'a str> {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        if let Ok(value) = value.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                if !token.trim().is_empty() {
                    return Some(token);
                }
            }
        }
    }
    query_token.filter(|token| !token.trim().is_empty())
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({ "error": "unauthorized" })),
    )
        .into_response()
}
