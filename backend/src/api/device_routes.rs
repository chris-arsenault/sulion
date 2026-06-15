//! Device pairing + device-token auth for external extensions.
//!
//! Flow (OAuth device-authorization shape; see the contract in the
//! `ableton-extensions` repo, `docs/sulion-api.md`):
//!
//! 1. `POST /api/devices/pair` (public) — a device gets a `device_code` (secret)
//!    and a short `user_code` (shown to the human).
//! 2. The user opens the browser, where they're already logged in, and
//!    `POST /api/devices/pair/approve` (Cognito-authenticated) binds the pairing
//!    to their identity.
//! 3. `POST /api/devices/pair/token` (public, polled by the device) mints a
//!    long-lived opaque token once approved.
//!
//! Only base64(SHA-256) hashes of the `device_code` and the minted token are
//! stored; plaintext exists only in transit.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use base64::Engine;
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use super::routes::{ApiError, ApiResult};
use crate::auth::AuthenticatedUser;
use crate::AppState;

const PAIRING_TTL_SECONDS: i32 = 15 * 60;
const POLL_INTERVAL_SECONDS: i64 = 2;
const DEFAULT_PUBLIC_URL: &str = "http://localhost:5173";
/// Display alphabet for `user_code`, minus visually ambiguous characters
/// (no `0/O`, `1/I`).
const USER_CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const USER_CODE_RETRIES: u32 = 5;

/// Public (unauthenticated) pairing endpoints — the device has no token yet.
pub(super) fn public_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/devices/pair", post(start_pairing))
        .route("/api/devices/pair/token", post(poll_token))
}

/// Approval endpoint — mounted inside the Cognito-authenticated group so we know
/// which logged-in user is approving.
pub(super) fn approve_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/devices/pair/approve", post(approve_pairing))
}

// ─── POST /api/devices/pair ──────────────────────────────────────────────

#[derive(Deserialize)]
struct StartPairingRequest {
    #[serde(default)]
    client: Option<String>,
}

#[derive(Serialize)]
struct StartPairingResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    expires_in: i32,
    interval: i64,
}

async fn start_pairing(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<StartPairingRequest>,
) -> ApiResult<Json<StartPairingResponse>> {
    let client = body
        .client
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let mut attempts = 0;
    loop {
        let device_code = random_token();
        let user_code = generate_user_code();
        let device_code_hash = hash_secret(&device_code);

        let res = sqlx::query(
            "INSERT INTO device_pairings \
               (device_code_hash, user_code, client, status, expires_at) \
             VALUES ($1, $2, $3, 'pending', NOW() + (INTERVAL '1 second' * $4))",
        )
        .bind(&device_code_hash)
        .bind(&user_code)
        .bind(&client)
        .bind(PAIRING_TTL_SECONDS)
        .execute(&state.pool)
        .await;

        match res {
            Ok(_) => {
                let base = public_base_url(&headers);
                let verification_uri = format!("{base}/pair");
                let verification_uri_complete = format!("{base}/pair?code={user_code}");
                return Ok(Json(StartPairingResponse {
                    device_code,
                    user_code,
                    verification_uri,
                    verification_uri_complete,
                    expires_in: PAIRING_TTL_SECONDS,
                    interval: POLL_INTERVAL_SECONDS,
                }));
            }
            Err(err) if is_unique_violation(&err) && attempts < USER_CODE_RETRIES => {
                attempts += 1;
                continue;
            }
            Err(err) => return Err(ApiError::Db(err)),
        }
    }
}

// ─── POST /api/devices/pair/token ────────────────────────────────────────

#[derive(Deserialize)]
struct PollTokenRequest {
    device_code: String,
}

async fn poll_token(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PollTokenRequest>,
) -> ApiResult<Response> {
    let hash = hash_secret(body.device_code.trim());
    let mut tx = state.pool.begin().await.map_err(ApiError::Db)?;

    let row = sqlx::query(
        "SELECT id, status, user_sub, client, (expires_at <= NOW()) AS expired \
           FROM device_pairings WHERE device_code_hash = $1 FOR UPDATE",
    )
    .bind(&hash)
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::Db)?;

    let Some(row) = row else {
        return Ok(gone("unknown device code"));
    };

    let id: i64 = row.get("id");
    let status: String = row.get("status");
    let expired: bool = row.get("expired");

    match status.as_str() {
        "claimed" => return Ok(gone("device code already claimed")),
        "denied" => return Ok(gone("pairing was denied")),
        _ if expired => {
            sqlx::query(
                "UPDATE device_pairings SET status = 'expired', updated_at = NOW() WHERE id = $1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(ApiError::Db)?;
            tx.commit().await.map_err(ApiError::Db)?;
            return Ok(gone("pairing expired"));
        }
        "pending" => return Ok(pending()),
        "approved" => {}
        other => {
            tracing::error!(status = other, "unexpected pairing status");
            return Err(ApiError::Internal(anyhow::anyhow!(
                "unexpected pairing status {other}"
            )));
        }
    }

    // Approved: mint the token now so plaintext is never persisted.
    let user_sub: String = row.get("user_sub");
    let client: String = row.get("client");
    let token = random_token();
    let token_hash = hash_secret(&token);

    let token_id: i64 = sqlx::query_scalar(
        "INSERT INTO device_tokens (token_hash, user_sub, client, label) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(&token_hash)
    .bind(&user_sub)
    .bind(&client)
    .bind(format!("paired {client}"))
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::Db)?;

    sqlx::query("UPDATE device_pairings SET status = 'claimed', updated_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::Db)?;
    tx.commit().await.map_err(ApiError::Db)?;

    tracing::info!(token_id, %client, "device token minted");
    Ok(json_status(
        StatusCode::OK,
        json!({ "access_token": token, "token_type": "Bearer" }),
    ))
}

// ─── POST /api/devices/pair/approve (authenticated) ──────────────────────

#[derive(Deserialize)]
struct ApproveRequest {
    user_code: String,
}

#[derive(Serialize)]
struct ApproveResponse {
    status: &'static str,
    client: String,
    user_code: String,
}

async fn approve_pairing(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<ApproveRequest>,
) -> ApiResult<Json<ApproveResponse>> {
    let user_code = body.user_code.trim().to_uppercase();
    if user_code.is_empty() {
        return Err(ApiError::BadRequest("user_code is required".into()));
    }

    let mut tx = state.pool.begin().await.map_err(ApiError::Db)?;
    let row = sqlx::query(
        "SELECT id, status, client, (expires_at <= NOW()) AS expired \
           FROM device_pairings WHERE user_code = $1 FOR UPDATE",
    )
    .bind(&user_code)
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::Db)?;

    let Some(row) = row else {
        return Err(ApiError::NotFound);
    };
    let id: i64 = row.get("id");
    let status: String = row.get("status");
    let client: String = row.get("client");
    let expired: bool = row.get("expired");

    let response = ApproveResponse {
        status: "approved",
        client: client.clone(),
        user_code: user_code.clone(),
    };

    match status.as_str() {
        "approved" => return Ok(Json(response)), // idempotent re-approval
        "claimed" => return Err(ApiError::BadRequest("pairing already used".into())),
        "denied" => return Err(ApiError::BadRequest("pairing was denied".into())),
        "pending" if expired => return Err(ApiError::BadRequest("pairing code expired".into())),
        "pending" => {}
        _ => return Err(ApiError::NotFound),
    }

    sqlx::query(
        "UPDATE device_pairings \
            SET status = 'approved', user_sub = $2, updated_at = NOW() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(&user.sub)
    .execute(&mut *tx)
    .await
    .map_err(ApiError::Db)?;
    tx.commit().await.map_err(ApiError::Db)?;

    tracing::info!(%client, user = %user.sub, "device pairing approved");
    Ok(Json(response))
}

// ─── device-token middleware ─────────────────────────────────────────────

/// Identity attached to a request authenticated by a device token. Inserted by
/// [`require_device_token`] and extracted by device-scoped handlers.
#[derive(Debug, Clone)]
pub(super) struct DevicePrincipal {
    pub token_id: i64,
    pub user_sub: String,
}

/// Auth layer for device-token routes (e.g. repo content ingest). Validates the bearer
/// token against `device_tokens`, touches `last_used_at`, and inserts a
/// [`DevicePrincipal`]. This is *separate* from `require_http_auth`, which
/// validates Cognito JWTs.
pub(super) async fn require_device_token(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(token) = bearer(req.headers()) else {
        return unauthorized();
    };
    let hash = hash_secret(token);

    let lookup = sqlx::query(
        "SELECT id, user_sub FROM device_tokens WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await;

    match lookup {
        Ok(Some(row)) => {
            let token_id: i64 = row.get("id");
            let user_sub: String = row.get("user_sub");
            // Best-effort usage timestamp; failure here must not block ingest.
            let _ = sqlx::query("UPDATE device_tokens SET last_used_at = NOW() WHERE id = $1")
                .bind(token_id)
                .execute(&state.pool)
                .await;
            req.extensions_mut()
                .insert(DevicePrincipal { token_id, user_sub });
            next.run(req).await
        }
        Ok(None) => unauthorized(),
        Err(err) => {
            tracing::error!(%err, "device token lookup failed");
            json_status(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "internal error" }),
            )
        }
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

fn random_bytes(n: usize) -> Vec<u8> {
    let rng = SystemRandom::new();
    let mut buf = vec![0u8; n];
    rng.fill(&mut buf).expect("system RNG available");
    buf
}

fn random_token() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes(32))
}

fn hash_secret(secret: &str) -> String {
    let digest = digest::digest(&digest::SHA256, secret.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.as_ref())
}

fn generate_user_code() -> String {
    let raw = random_bytes(8);
    let c: Vec<char> = raw
        .iter()
        .map(|b| USER_CODE_ALPHABET[(*b as usize) % USER_CODE_ALPHABET.len()] as char)
        .collect();
    format!(
        "{}{}{}{}-{}{}{}{}",
        c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]
    )
}

/// Public origin the browser-facing `/pair` link should point at. The pairing
/// request reaches us through nginx, which forwards the real `Host`, so we can
/// derive it instead of hard-coding a host. Precedence: explicit
/// `SULION_PUBLIC_URL` override → request headers (`X-Forwarded-Host`/`Host`,
/// `X-Forwarded-Proto` or https for a non-local host) → localhost only as a
/// last resort when there is no Host at all (a raw client, never a browser).
fn public_base_url(headers: &HeaderMap) -> String {
    derive_base_url(headers, std::env::var("SULION_PUBLIC_URL").ok().as_deref())
}

fn derive_base_url(headers: &HeaderMap, env_override: Option<&str>) -> String {
    if let Some(base) = env_override
        .map(|v| v.trim().trim_end_matches('/'))
        .filter(|v| !v.is_empty())
    {
        return base.to_string();
    }

    let host = forwarded(headers, "x-forwarded-host")
        .or_else(|| header_value(headers, header::HOST.as_str()));
    match host {
        Some(host) => {
            let scheme = forwarded(headers, "x-forwarded-proto")
                .unwrap_or(if is_local_host(host) { "http" } else { "https" });
            format!("{scheme}://{host}")
        }
        None => DEFAULT_PUBLIC_URL.to_string(),
    }
}

/// First value of a (possibly comma-listed) forwarded header, trimmed; None if empty.
fn forwarded<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let raw = header_value(headers, name)?;
    let first = raw.split(',').next().unwrap_or(raw).trim();
    (!first.is_empty()).then_some(first)
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)?
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn is_local_host(host: &str) -> bool {
    let h = host.split(':').next().unwrap_or(host);
    h == "localhost" || h == "127.0.0.1" || h == "::1" || h.ends_with(".local")
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .map(|db| db.is_unique_violation())
        .unwrap_or(false)
}

fn json_status(status: StatusCode, body: serde_json::Value) -> Response {
    (status, Json(body)).into_response()
}

fn pending() -> Response {
    json_status(
        StatusCode::PRECONDITION_REQUIRED,
        json!({ "error": "authorization_pending" }),
    )
}

fn gone(msg: &str) -> Response {
    json_status(StatusCode::GONE, json!({ "error": msg }))
}

fn unauthorized() -> Response {
    json_status(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }))
}

#[cfg(test)]
mod tests {
    use super::derive_base_url;
    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn explicit_override_wins() {
        let h = headers(&[("host", "ignored.example.com")]);
        assert_eq!(
            derive_base_url(&h, Some("https://sulion.example.com/")),
            "https://sulion.example.com",
        );
    }

    #[test]
    fn derives_https_from_host_for_real_deployments() {
        let h = headers(&[("host", "sulion.ahara.dev")]);
        assert_eq!(derive_base_url(&h, None), "https://sulion.ahara.dev");
    }

    #[test]
    fn local_host_stays_http() {
        let h = headers(&[("host", "127.0.0.1:8080")]);
        assert_eq!(derive_base_url(&h, None), "http://127.0.0.1:8080");
    }

    #[test]
    fn honors_forwarded_proto_and_host() {
        let h = headers(&[
            ("host", "internal-backend"),
            ("x-forwarded-host", "sulion.ahara.dev"),
            ("x-forwarded-proto", "https"),
        ]);
        assert_eq!(derive_base_url(&h, None), "https://sulion.ahara.dev");
    }

    #[test]
    fn forwarded_takes_first_of_a_list() {
        let h = headers(&[("x-forwarded-host", "sulion.ahara.dev, proxy.internal")]);
        assert_eq!(derive_base_url(&h, None), "https://sulion.ahara.dev");
    }

    #[test]
    fn blank_override_falls_through_to_headers() {
        let h = headers(&[("host", "sulion.ahara.dev")]);
        assert_eq!(derive_base_url(&h, Some("   ")), "https://sulion.ahara.dev");
    }

    #[test]
    fn no_host_falls_back_to_default() {
        assert_eq!(
            derive_base_url(&HeaderMap::new(), None),
            super::DEFAULT_PUBLIC_URL
        );
    }
}
