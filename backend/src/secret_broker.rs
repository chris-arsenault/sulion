use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use axum::extract::{Extension, Path, Query, Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::prelude::{Engine as _, BASE64_STANDARD};
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::AeadCore;
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use ring::signature;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

use crate::auth::{AccessTokenQuery, AuthState, AuthenticatedUser};
use crate::db::{self, Pool};
use crate::secret_protocol::{
    canonical_use_payload, RegisterPtyCredentialRequest, SignedUseSecretRequest, UseSecretResponse,
};

#[derive(Debug, Clone)]
pub struct BrokerConfig {
    pub listen: SocketAddr,
    pub db_url: String,
    pub master_key_path: PathBuf,
    pub auth_issuer_url: String,
    pub auth_client_id: String,
    pub registration_token: String,
}

impl BrokerConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen = std::env::var("SULION_SECRET_BROKER_LISTEN")
            .unwrap_or_else(|_| "0.0.0.0:8081".to_string())
            .parse()
            .context("invalid SULION_SECRET_BROKER_LISTEN")?;
        let db_url = std::env::var("SULION_SECRET_BROKER_DB_URL")
            .map_err(|_| anyhow!("SULION_SECRET_BROKER_DB_URL must be set"))?;
        let master_key_path = PathBuf::from(
            std::env::var("SULION_SECRET_BROKER_MASTER_KEY_PATH")
                .unwrap_or_else(|_| "/var/lib/sulion-broker/master.key".to_string()),
        );
        let auth_issuer_url = std::env::var("SULION_AUTH_ISSUER_URL")
            .map_err(|_| anyhow!("SULION_AUTH_ISSUER_URL must be set for broker auth"))?;
        let auth_client_id = std::env::var("SULION_AUTH_CLIENT_ID")
            .map_err(|_| anyhow!("SULION_AUTH_CLIENT_ID must be set for broker auth"))?;
        let registration_token = std::env::var("SULION_SECRET_BROKER_REGISTRATION_TOKEN")
            .map_err(|_| anyhow!("SULION_SECRET_BROKER_REGISTRATION_TOKEN must be set"))?;
        Ok(Self {
            listen,
            db_url,
            master_key_path,
            auth_issuer_url,
            auth_client_id,
            registration_token,
        })
    }
}

#[derive(Clone)]
pub struct BrokerState {
    pub pool: Pool,
    auth: Arc<AuthState>,
    crypto: Arc<SecretCrypto>,
    registration_token: String,
}

impl BrokerState {
    pub async fn from_config(config: &BrokerConfig) -> anyhow::Result<Arc<Self>> {
        let pool = db::connect(&config.db_url).await?;
        sqlx::migrate!("./broker_migrations").run(&pool).await?;
        let auth = Arc::new(AuthState::new(crate::config::AuthConfig {
            issuer_url: config.auth_issuer_url.clone(),
            client_id: config.auth_client_id.clone(),
        }));
        let crypto = Arc::new(SecretCrypto::from_file(&config.master_key_path).await?);
        Ok(Arc::new(Self {
            pool,
            auth,
            crypto,
            registration_token: config.registration_token.clone(),
        }))
    }
}

pub fn app(state: Arc<BrokerState>) -> Router {
    let user_routes = Router::new()
        .route("/health", get(health))
        .route("/v1/secrets", get(list_secrets))
        .route(
            "/v1/secrets/:id",
            get(get_secret).put(upsert_secret).delete(delete_secret),
        )
        .route(
            "/v1/grants",
            get(list_grants).post(unlock_grant).delete(revoke_grant),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_user_auth,
        ));

    let use_routes = Router::new().route("/v1/use", post(use_secret));

    let registration_routes = Router::new()
        .route("/v1/pty-credentials", post(register_pty_credential))
        .route("/v1/pty-credentials/:id", delete(revoke_pty_credential))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_registration_auth,
        ));

    Router::new()
        .merge(user_routes)
        .merge(use_routes)
        .merge(registration_routes)
        .layer(
            CorsLayer::new()
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
                .allow_origin(Any),
        )
        .with_state(state)
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

async fn require_user_auth(
    State(state): State<Arc<BrokerState>>,
    query: Query<AccessTokenQuery>,
    mut req: Request,
    next: Next,
) -> Response {
    let token = bearer_from_request(req.headers(), query.access_token.as_deref());
    let Some(token) = token else {
        return unauthorized();
    };
    match state.auth.validate_bearer(token).await {
        Ok(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(err) => {
            tracing::warn!(error = %err, "broker authentication failed");
            unauthorized()
        }
    }
}

async fn require_registration_auth(
    State(state): State<Arc<BrokerState>>,
    query: Query<AccessTokenQuery>,
    req: Request,
    next: Next,
) -> Response {
    let token = bearer_from_request(req.headers(), query.access_token.as_deref());
    if token != Some(state.registration_token.as_str()) {
        return unauthorized();
    }
    next.run(req).await
}

#[derive(Debug, Deserialize, Serialize)]
struct SecretEnvelope {
    description: String,
    scope: String,
    repo: Option<String>,
    env: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
struct SecretMetadata {
    id: String,
    description: String,
    scope: String,
    repo: Option<String>,
    env_keys: Vec<String>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

async fn list_secrets(
    State(state): State<Arc<BrokerState>>,
) -> Result<Json<Vec<SecretMetadata>>, BrokerError> {
    let rows = sqlx::query(
        "SELECT id, description, scope, repo, ciphertext, nonce, updated_at \
         FROM secret_broker.secrets ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let ciphertext: Vec<u8> = row.get("ciphertext");
        let nonce: Vec<u8> = row.get("nonce");
        let env = state.crypto.decrypt_env(&ciphertext, &nonce)?;
        let mut env_keys = env.into_keys().collect::<Vec<_>>();
        env_keys.sort();
        items.push(SecretMetadata {
            id: row.get("id"),
            description: row.get("description"),
            scope: row.get("scope"),
            repo: row.get("repo"),
            env_keys,
            updated_at: row.get("updated_at"),
        });
    }
    Ok(Json(items))
}

async fn upsert_secret(
    State(state): State<Arc<BrokerState>>,
    Path(id): Path<String>,
    Json(body): Json<SecretEnvelope>,
) -> Result<StatusCode, BrokerError> {
    validate_secret_id(&id)?;
    if body.env.is_empty() {
        return Err(BrokerError::bad_request("env set must not be empty"));
    }
    let existing = load_secret_env(&state, &id).await?;
    let mut env = HashMap::with_capacity(body.env.len());
    for (key, value) in body.env {
        if value.is_empty() {
            if let Some(existing_value) = existing.as_ref().and_then(|items| items.get(&key)) {
                env.insert(key, existing_value.clone());
                continue;
            }
            return Err(BrokerError::bad_request(format!(
                "value for new env var {key} must not be empty"
            )));
        }
        env.insert(key, value);
    }
    if env.is_empty() {
        return Err(BrokerError::bad_request("env set must not be empty"));
    }
    let (ciphertext, nonce) = state.crypto.encrypt_env(&env)?;
    sqlx::query(
        "INSERT INTO secret_broker.secrets \
         (id, description, scope, repo, ciphertext, nonce, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, NOW()) \
         ON CONFLICT (id) DO UPDATE SET \
           description = EXCLUDED.description, \
           scope = EXCLUDED.scope, \
           repo = EXCLUDED.repo, \
           ciphertext = EXCLUDED.ciphertext, \
           nonce = EXCLUDED.nonce, \
           updated_at = NOW()",
    )
    .bind(id)
    .bind(body.description)
    .bind(body.scope)
    .bind(body.repo)
    .bind(ciphertext)
    .bind(nonce)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_secret(
    State(state): State<Arc<BrokerState>>,
    Path(id): Path<String>,
) -> Result<Json<SecretEnvelope>, BrokerError> {
    validate_secret_id(&id)?;
    let row = sqlx::query(
        "SELECT description, scope, repo, ciphertext, nonce \
         FROM secret_broker.secrets WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Err(BrokerError {
            status: StatusCode::NOT_FOUND,
            message: "secret not found".to_string(),
        });
    };
    let ciphertext: Vec<u8> = row.get("ciphertext");
    let nonce: Vec<u8> = row.get("nonce");
    let env = state
        .crypto
        .decrypt_env(&ciphertext, &nonce)?
        .into_keys()
        .map(|key| (key, String::new()))
        .collect();
    Ok(Json(SecretEnvelope {
        description: row.get("description"),
        scope: row.get("scope"),
        repo: row.get("repo"),
        env,
    }))
}

async fn load_secret_env(
    state: &BrokerState,
    id: &str,
) -> Result<Option<HashMap<String, String>>, BrokerError> {
    let row = sqlx::query("SELECT ciphertext, nonce FROM secret_broker.secrets WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let ciphertext: Vec<u8> = row.get("ciphertext");
    let nonce: Vec<u8> = row.get("nonce");
    Ok(Some(state.crypto.decrypt_env(&ciphertext, &nonce)?))
}

async fn delete_secret(
    State(state): State<Arc<BrokerState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, BrokerError> {
    sqlx::query("DELETE FROM secret_broker.secrets WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct GrantsQuery {
    pty_session_id: Uuid,
}

#[derive(Debug, Serialize)]
struct GrantMetadata {
    secret_id: String,
    tool: String,
    granted_by_sub: String,
    granted_by_username: Option<String>,
    expires_at: chrono::DateTime<chrono::Utc>,
}

async fn list_grants(
    State(state): State<Arc<BrokerState>>,
    Query(query): Query<GrantsQuery>,
) -> Result<Json<Vec<GrantMetadata>>, BrokerError> {
    let rows = sqlx::query(
        "SELECT secret_id, tool, granted_by_sub, granted_by_username, expires_at \
         FROM secret_broker.grants \
         WHERE pty_session_id = $1 AND revoked_at IS NULL AND expires_at > NOW() \
         ORDER BY expires_at DESC",
    )
    .bind(query.pty_session_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| GrantMetadata {
                secret_id: row.get("secret_id"),
                tool: row.get("tool"),
                granted_by_sub: row.get("granted_by_sub"),
                granted_by_username: row.get("granted_by_username"),
                expires_at: row.get("expires_at"),
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
struct GrantRequest {
    pty_session_id: Uuid,
    secret_id: String,
    tool: String,
    ttl_seconds: i64,
}

async fn unlock_grant(
    State(state): State<Arc<BrokerState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<GrantRequest>,
) -> Result<StatusCode, BrokerError> {
    validate_secret_id(&body.secret_id)?;
    validate_tool_name(&body.tool)?;
    if !(60..=86_400).contains(&body.ttl_seconds) {
        return Err(BrokerError::bad_request(
            "ttl_seconds must be between 60 and 86400",
        ));
    }
    sqlx::query(
        "INSERT INTO secret_broker.grants \
         (id, pty_session_id, secret_id, tool, granted_by_sub, granted_by_username, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, NOW() + make_interval(secs => $7::int))",
    )
    .bind(Uuid::new_v4())
    .bind(body.pty_session_id)
    .bind(body.secret_id)
    .bind(body.tool)
    .bind(user.sub)
    .bind(user.username)
    .bind(body.ttl_seconds as i32)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::CREATED)
}

#[derive(Debug, Deserialize)]
struct RevokeGrantRequest {
    pty_session_id: Uuid,
    secret_id: String,
    tool: String,
}

async fn revoke_grant(
    State(state): State<Arc<BrokerState>>,
    Json(body): Json<RevokeGrantRequest>,
) -> Result<StatusCode, BrokerError> {
    sqlx::query(
        "UPDATE secret_broker.grants \
         SET revoked_at = NOW() \
         WHERE pty_session_id = $1 AND secret_id = $2 AND tool = $3 AND revoked_at IS NULL",
    )
    .bind(body.pty_session_id)
    .bind(body.secret_id)
    .bind(body.tool)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn register_pty_credential(
    State(state): State<Arc<BrokerState>>,
    Json(body): Json<RegisterPtyCredentialRequest>,
) -> Result<StatusCode, BrokerError> {
    let public_key = BASE64_STANDARD
        .decode(body.public_key.as_bytes())
        .map_err(|_| BrokerError::bad_request("invalid public key"))?;
    if public_key.len() != 32 {
        return Err(BrokerError::bad_request("invalid public key length"));
    }
    sqlx::query(
        "INSERT INTO secret_broker.pty_credentials \
         (pty_session_id, public_key, created_at, revoked_at) \
         VALUES ($1, $2, NOW(), NULL) \
         ON CONFLICT (pty_session_id) DO UPDATE SET \
           public_key = EXCLUDED.public_key, \
           created_at = NOW(), \
           revoked_at = NULL",
    )
    .bind(body.pty_session_id)
    .bind(body.public_key)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::CREATED)
}

async fn revoke_pty_credential(
    State(state): State<Arc<BrokerState>>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, BrokerError> {
    sqlx::query(
        "UPDATE secret_broker.pty_credentials \
         SET revoked_at = NOW() \
         WHERE pty_session_id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn use_secret(
    State(state): State<Arc<BrokerState>>,
    Json(body): Json<SignedUseSecretRequest>,
) -> Result<Json<UseSecretResponse>, BrokerError> {
    verify_signed_use_request(&state, &body).await?;
    validate_tool_name(&body.tool)?;
    if body.tool == "aws" && body.secret_id.is_none() {
        return Err(BrokerError::bad_request(
            "aws redemption requires a secret id",
        ));
    }
    let rows = if let Some(secret_id) = &body.secret_id {
        validate_secret_id(secret_id)?;
        sqlx::query(
            "SELECT s.id, s.ciphertext, s.nonce \
             FROM secret_broker.grants g \
             JOIN secret_broker.secrets s ON s.id = g.secret_id \
             WHERE g.pty_session_id = $1 \
               AND g.secret_id = $2 \
               AND g.tool = $3 \
               AND g.revoked_at IS NULL \
               AND g.expires_at > NOW() \
             ORDER BY g.expires_at DESC \
             LIMIT 1",
        )
        .bind(body.pty_session_id)
        .bind(secret_id)
        .bind(&body.tool)
        .fetch_all(&state.pool)
        .await?
    } else {
        sqlx::query(
            "SELECT DISTINCT ON (s.id) s.id, s.ciphertext, s.nonce, g.expires_at \
             FROM secret_broker.grants g \
             JOIN secret_broker.secrets s ON s.id = g.secret_id \
             WHERE g.pty_session_id = $1 \
               AND g.tool = $2 \
               AND g.revoked_at IS NULL \
               AND g.expires_at > NOW() \
             ORDER BY s.id, g.expires_at DESC",
        )
        .bind(body.pty_session_id)
        .bind(&body.tool)
        .fetch_all(&state.pool)
        .await?
    };
    if rows.is_empty() {
        return Err(BrokerError::forbidden(
            "secret is not unlocked for this terminal/tool",
        ));
    }
    let mut env = HashMap::new();
    let mut granted_secret_ids = Vec::with_capacity(rows.len());
    for row in rows {
        let secret_id: String = row.get("id");
        let ciphertext: Vec<u8> = row.get("ciphertext");
        let nonce: Vec<u8> = row.get("nonce");
        let secret_env = state.crypto.decrypt_env(&ciphertext, &nonce)?;
        for (key, value) in secret_env {
            if env.insert(key.clone(), value).is_some() {
                return Err(BrokerError::bad_request(format!(
                    "conflicting env var {key} across unlocked secrets for tool {}",
                    body.tool
                )));
            }
        }
        granted_secret_ids.push(secret_id);
    }
    for secret_id in granted_secret_ids {
        sqlx::query(
            "INSERT INTO secret_broker.use_audit \
             (id, pty_session_id, secret_id, tool, used_at) VALUES ($1, $2, $3, $4, NOW())",
        )
        .bind(Uuid::new_v4())
        .bind(body.pty_session_id)
        .bind(secret_id)
        .bind(&body.tool)
        .execute(&state.pool)
        .await?;
    }
    Ok(Json(UseSecretResponse { env }))
}

async fn verify_signed_use_request(
    state: &BrokerState,
    body: &SignedUseSecretRequest,
) -> Result<(), BrokerError> {
    if body.nonce.trim().is_empty() || body.nonce.len() > 128 {
        return Err(BrokerError::unauthorized("invalid nonce"));
    }
    let now = chrono::Utc::now().timestamp();
    if (body.timestamp_unix_seconds - now).abs() > 60 {
        return Err(BrokerError::unauthorized("stale secret-use request"));
    }
    let row = sqlx::query(
        "SELECT public_key \
         FROM secret_broker.pty_credentials \
         WHERE pty_session_id = $1 AND revoked_at IS NULL",
    )
    .bind(body.pty_session_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Err(BrokerError::unauthorized("unknown PTY credential"));
    };
    let public_key: String = row.get("public_key");
    let public_key = BASE64_STANDARD
        .decode(public_key.as_bytes())
        .map_err(|_| BrokerError::unauthorized("invalid registered PTY key"))?;
    let signature = BASE64_STANDARD
        .decode(body.signature.as_bytes())
        .map_err(|_| BrokerError::unauthorized("invalid request signature"))?;
    let canonical = canonical_use_payload(
        body.pty_session_id,
        body.secret_id.as_deref(),
        &body.tool,
        body.timestamp_unix_seconds,
        &body.nonce,
    );
    signature::UnparsedPublicKey::new(&signature::ED25519, public_key)
        .verify(canonical.as_bytes(), &signature)
        .map_err(|_| BrokerError::unauthorized("invalid request signature"))?;

    sqlx::query(
        "DELETE FROM secret_broker.pty_use_nonces WHERE seen_at < NOW() - INTERVAL '5 minutes'",
    )
    .execute(&state.pool)
    .await?;
    let inserted = sqlx::query(
        "INSERT INTO secret_broker.pty_use_nonces (pty_session_id, nonce, seen_at) \
         VALUES ($1, $2, NOW()) \
         ON CONFLICT DO NOTHING",
    )
    .bind(body.pty_session_id)
    .bind(&body.nonce)
    .execute(&state.pool)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(BrokerError::unauthorized("replayed secret-use request"));
    }
    Ok(())
}

struct SecretCrypto {
    cipher: ChaCha20Poly1305,
}

impl SecretCrypto {
    async fn from_file(path: &PathBuf) -> anyhow::Result<Self> {
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("read master key {}", path.display()))?;
        let bytes = bytes
            .into_iter()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect::<Vec<_>>();
        if bytes.len() != 32 {
            return Err(anyhow!(
                "master key at {} must be exactly 32 bytes",
                path.display()
            ));
        }
        let key = Key::from_slice(&bytes);
        Ok(Self {
            cipher: ChaCha20Poly1305::new(key),
        })
    }

    fn encrypt_env(&self, env: &HashMap<String, String>) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let plaintext = serde_json::to_vec(env)?;
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_ref())
            .map_err(|_| anyhow!("encrypt secret payload"))?;
        Ok((ciphertext, nonce.to_vec()))
    }

    fn decrypt_env(
        &self,
        ciphertext: &[u8],
        nonce: &[u8],
    ) -> anyhow::Result<HashMap<String, String>> {
        if nonce.len() != 12 {
            return Err(anyhow!("invalid nonce length"));
        }
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| anyhow!("decrypt secret payload"))?;
        Ok(serde_json::from_slice(&plaintext)?)
    }
}

#[derive(Debug)]
pub struct BrokerError {
    status: StatusCode,
    message: String,
}

impl BrokerError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
        }
    }
}

impl From<sqlx::Error> for BrokerError {
    fn from(value: sqlx::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: value.to_string(),
        }
    }
}

impl From<anyhow::Error> for BrokerError {
    fn from(value: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: value.to_string(),
        }
    }
}

impl IntoResponse for BrokerError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
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
        Json(serde_json::json!({ "error": "unauthorized" })),
    )
        .into_response()
}

fn validate_secret_id(id: &str) -> Result<(), BrokerError> {
    if id.is_empty()
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(BrokerError::bad_request("invalid secret id"));
    }
    Ok(())
}

fn validate_tool_name(tool: &str) -> Result<(), BrokerError> {
    if !matches!(tool, "with-cred" | "aws") {
        return Err(BrokerError::bad_request(
            "tool must be one of: with-cred, aws",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_crypto_round_trips_encrypted_env_payloads() {
        let key = Key::from_slice(&[7_u8; 32]);
        let crypto = SecretCrypto {
            cipher: ChaCha20Poly1305::new(key),
        };
        let env = HashMap::from([
            ("ANTHROPIC_API_KEY".to_string(), "sk-ant-test".to_string()),
            ("CLAUDE_API_KEY".to_string(), "claude-test".to_string()),
        ]);

        let (ciphertext, nonce) = crypto.encrypt_env(&env).expect("encrypt env");

        assert_eq!(nonce.len(), 12);
        assert_ne!(ciphertext, serde_json::to_vec(&env).expect("serialize env"));
        assert_eq!(
            crypto
                .decrypt_env(&ciphertext, &nonce)
                .expect("decrypt env"),
            env
        );
    }

    #[test]
    fn validation_allows_only_supported_secret_tools() {
        assert!(validate_tool_name("with-cred").is_ok());
        assert!(validate_tool_name("aws").is_ok());

        let err = validate_tool_name("shell").expect_err("unsupported tool");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn validation_rejects_secret_ids_that_cannot_be_path_or_query_safe() {
        assert!(validate_secret_id("anthropic.api-key_1").is_ok());

        for id in ["", "../aws", "aws default", "aws/default"] {
            let err = validate_secret_id(id).expect_err("invalid secret id");
            assert_eq!(err.status, StatusCode::BAD_REQUEST);
        }
    }
}
