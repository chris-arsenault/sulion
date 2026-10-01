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

use crate::auth::{AuthState, AuthenticatedUser};
use crate::db::{self, Pool};
use crate::secret_protocol::{
    canonical_use_payload, RegisterPtyCredentialRequest, SignedUseSecretRequest, UseSecretResponse,
};

mod browser_auth;
mod redeem;
use browser_auth::{check_browser_authority, revoke_browser_principal, revoke_browser_session};
use redeem::use_secret;

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
        let auth = crate::config::AuthConfig::from_env()?
            .ok_or_else(|| anyhow!("broker authentication cannot be disabled"))?;
        let auth_issuer_url = auth.issuer_url;
        let auth_client_id = auth.client_id;
        let registration_token = std::env::var("SULION_SECRET_BROKER_REGISTRATION_TOKEN")
            .map_err(|_| anyhow!("SULION_SECRET_BROKER_REGISTRATION_TOKEN must be set"))?;
        anyhow::ensure!(
            !registration_token.trim().is_empty(),
            "broker registration token must not be blank"
        );
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

fn user_routes() -> Router<Arc<BrokerState>> {
    Router::new()
        .route("/v1/session/revoke", post(revoke_browser_session))
        .route("/v1/secrets", get(list_secrets))
        .route(
            "/v1/secrets/:id",
            get(get_secret).put(upsert_secret).delete(delete_secret),
        )
        .route(
            "/v1/grants",
            get(list_grants).post(unlock_grant).delete(revoke_grant),
        )
}

/// Integration tests supply a principal while exercising the real management handlers.
#[cfg(feature = "integration-tests")]
pub fn management_app_for_tests(state: Arc<BrokerState>) -> Router {
    user_routes()
        .layer(Extension(AuthenticatedUser::dev()))
        .with_state(state)
}

pub fn app(state: Arc<BrokerState>) -> Router {
    let user_routes = user_routes().route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        require_user_auth,
    ));

    let use_routes = Router::new().route("/v1/use", post(use_secret));

    let registration_routes = Router::new()
        .route("/v1/auth/check", post(check_browser_authority))
        .route("/v1/auth/revoke", post(revoke_browser_principal))
        .route("/v1/pty-credentials", post(register_pty_credential))
        .route("/v1/pty-credentials/:id", delete(revoke_pty_credential))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_registration_auth,
        ));

    Router::new()
        // Liveness only, static body, no state read — deliberately outside
        // the auth layers so orchestration (compose healthchecks, the e2e
        // stack) can gate on it without a token.
        .route("/health", get(health))
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
    mut req: Request,
    next: Next,
) -> Response {
    let token = bearer_from_request(req.headers());
    let Some(token) = token else {
        return unauthorized();
    };
    match state.auth.validate_bearer(token).await {
        Ok(user) => {
            match crate::auth::revocation::allowed(&state.pool, &user.authority()).await {
                Ok(true) => {}
                // A lost sign-out response can be retried, but an old token
                // must never advance the cutoff and revoke a newer login.
                Ok(false)
                    if req.method() == Method::POST && req.uri().path() == "/v1/session/revoke" =>
                {
                    return StatusCode::NO_CONTENT.into_response();
                }
                _ => return unauthorized(),
            }
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
    req: Request,
    next: Next,
) -> Response {
    let token = bearer_from_request(req.headers());
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
    /// Programs this secret is injected into for every terminal, at the lowest
    /// precedence; absent when it has no all-terminals grant.
    all_terminal_programs: Option<Vec<String>>,
}

async fn list_secrets(
    State(state): State<Arc<BrokerState>>,
) -> Result<Json<Vec<SecretMetadata>>, BrokerError> {
    let rows = sqlx::query(
        "SELECT s.id, s.description, s.scope, s.repo, s.ciphertext, s.nonce, s.updated_at, \
                g.programs AS all_terminal_programs \
         FROM secret_broker.secrets s \
         LEFT JOIN secret_broker.grants g ON g.secret_id = s.id \
           AND g.scope = 'all_terminals' AND g.revoked_at IS NULL \
         ORDER BY s.id",
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
            all_terminal_programs: row.get("all_terminal_programs"),
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
    granted_by_sub: String,
    granted_by_username: Option<String>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    repo: Option<String>,
    /// `terminal`, `repository`, or `all_terminals`.
    scope: String,
    /// The programs an `all_terminals` grant applies to.
    programs: Option<Vec<String>>,
}

async fn list_grants(
    State(state): State<Arc<BrokerState>>,
    Query(query): Query<GrantsQuery>,
) -> Result<Json<Vec<GrantMetadata>>, BrokerError> {
    let rows = sqlx::query(
        "SELECT secret_id, granted_by_sub, granted_by_username, expires_at, repo, scope, \
                programs \
         FROM ( \
           SELECT DISTINCT ON (secret_id, scope) \
             secret_id, granted_by_sub, granted_by_username, expires_at, repo, scope, \
             programs \
           FROM secret_broker.effective_grants \
           WHERE pty_session_id = $1 \
           ORDER BY secret_id, scope, expires_at DESC \
         ) latest \
         ORDER BY expires_at DESC",
    )
    .bind(query.pty_session_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| GrantMetadata {
                secret_id: row.get("secret_id"),
                granted_by_sub: row.get("granted_by_sub"),
                granted_by_username: row.get("granted_by_username"),
                expires_at: row.get("expires_at"),
                repo: row.get("repo"),
                scope: row.get("scope"),
                programs: row.get("programs"),
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
struct GrantRequest {
    /// The terminal for `terminal` scope, or whose repository `repository`
    /// scope resolves; unused for `all_terminals`.
    pty_session_id: Option<Uuid>,
    secret_id: String,
    ttl_seconds: Option<i64>,
    #[serde(default)]
    scope: GrantScope,
    /// Program names an `all_terminals` grant applies to.
    programs: Option<Vec<String>>,
}

#[derive(Debug, Default, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum GrantScope {
    #[default]
    Terminal,
    Repository,
    AllTerminals,
}

impl GrantScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Repository => "repository",
            Self::AllTerminals => "all_terminals",
        }
    }
}

/// The grant row a request addresses: one active grant per secret and target.
struct GrantTarget {
    scope: GrantScope,
    pty_session_id: Option<Uuid>,
    repo: Option<String>,
}

impl GrantTarget {
    async fn resolve(
        state: &BrokerState,
        scope: GrantScope,
        pty_session_id: Option<Uuid>,
    ) -> Result<Self, BrokerError> {
        let pty =
            || pty_session_id.ok_or_else(|| BrokerError::bad_request("pty_session_id is required"));
        Ok(match scope {
            GrantScope::Terminal => Self {
                scope,
                pty_session_id: Some(pty()?),
                repo: None,
            },
            GrantScope::Repository => Self {
                scope,
                pty_session_id: None,
                repo: Some(registered_repo(state, pty()?).await?),
            },
            GrantScope::AllTerminals => Self {
                scope,
                pty_session_id: None,
                repo: None,
            },
        })
    }

    async fn revoke(
        &self,
        executor: impl sqlx::PgExecutor<'_>,
        secret_id: &str,
    ) -> Result<(), BrokerError> {
        sqlx::query(
            "UPDATE secret_broker.grants SET revoked_at = NOW() \
             WHERE secret_id = $1 AND scope = $2 AND revoked_at IS NULL \
               AND pty_session_id IS NOT DISTINCT FROM $3 AND repo IS NOT DISTINCT FROM $4",
        )
        .bind(secret_id)
        .bind(self.scope.as_str())
        .bind(self.pty_session_id)
        .bind(&self.repo)
        .execute(executor)
        .await?;
        Ok(())
    }
}

async fn registered_repo(state: &BrokerState, pty: Uuid) -> Result<String, BrokerError> {
    sqlx::query_scalar::<_, String>(
        "SELECT repo FROM secret_broker.pty_credentials \
         WHERE pty_session_id = $1 AND revoked_at IS NULL AND repo IS NOT NULL",
    )
    .bind(pty)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| BrokerError::bad_request("terminal has no registered repository"))
}

async fn unlock_grant(
    State(state): State<Arc<BrokerState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<GrantRequest>,
) -> Result<StatusCode, BrokerError> {
    validate_secret_id(&body.secret_id)?;
    let ttl_seconds = match (body.scope, body.ttl_seconds) {
        (GrantScope::Terminal, Some(ttl)) if (60..=86_400).contains(&ttl) => Some(ttl as i32),
        (GrantScope::Terminal, _) => {
            return Err(BrokerError::bad_request(
                "ttl_seconds must be between 60 and 86400",
            ))
        }
        (_, None) => None,
        (_, Some(_)) => {
            return Err(BrokerError::bad_request(
                "only terminal grants expire; omit ttl_seconds",
            ))
        }
    };
    let programs = match (body.scope, body.programs) {
        (GrantScope::AllTerminals, Some(programs)) => Some(redeem::validate_programs(programs)?),
        (GrantScope::AllTerminals, None) => {
            return Err(BrokerError::bad_request(
                "an all-terminals grant needs at least one program",
            ))
        }
        (_, None) => None,
        (_, Some(_)) => {
            return Err(BrokerError::bad_request(
                "only all-terminals grants take programs",
            ))
        }
    };
    let target = GrantTarget::resolve(&state, body.scope, body.pty_session_id).await?;
    let mut tx = state.pool.begin().await?;
    target.revoke(&mut *tx, &body.secret_id).await?;
    sqlx::query(
        "INSERT INTO secret_broker.grants \
         (id, scope, pty_session_id, repo, secret_id, granted_by_sub, granted_by_username, \
          expires_at, programs) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, \
                 NOW() + make_interval(secs => $8::int), $9)",
    )
    .bind(Uuid::new_v4())
    .bind(target.scope.as_str())
    .bind(target.pty_session_id)
    .bind(&target.repo)
    .bind(&body.secret_id)
    .bind(user.sub)
    .bind(user.username)
    .bind(ttl_seconds)
    .bind(programs)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::CREATED)
}

#[derive(Debug, Deserialize)]
struct RevokeGrantRequest {
    pty_session_id: Option<Uuid>,
    secret_id: String,
    #[serde(default)]
    scope: GrantScope,
}

async fn revoke_grant(
    State(state): State<Arc<BrokerState>>,
    Json(body): Json<RevokeGrantRequest>,
) -> Result<StatusCode, BrokerError> {
    let target = GrantTarget::resolve(&state, body.scope, body.pty_session_id).await?;
    target.revoke(&state.pool, &body.secret_id).await?;
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
    if body
        .repo
        .as_ref()
        .is_some_and(|repo| repo.trim().is_empty())
    {
        return Err(BrokerError::bad_request("repository must not be empty"));
    }
    sqlx::query(
        "INSERT INTO secret_broker.pty_credentials \
         (pty_session_id, public_key, repo, created_at, revoked_at) \
         VALUES ($1, $2, $3, NOW(), NULL) \
         ON CONFLICT (pty_session_id) DO UPDATE SET \
           public_key = EXCLUDED.public_key, \
           repo = EXCLUDED.repo, \
           created_at = NOW(), \
           revoked_at = NULL",
    )
    .bind(body.pty_session_id)
    .bind(body.public_key)
    .bind(body.repo)
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

#[cfg(test)]
mod tests;
