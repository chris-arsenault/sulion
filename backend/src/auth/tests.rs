use super::*;
use axum::response::IntoResponse;
use axum::{routing::get, Json, Router};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Fixture {
    auth: Arc<AuthState>,
    calls: Arc<AtomicUsize>,
    outage: Arc<AtomicBool>,
    slow: Arc<AtomicBool>,
    rotated: Arc<AtomicBool>,
    response_body: Arc<std::sync::Mutex<Option<String>>>,
    revoked: Arc<AtomicBool>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let outage = Arc::new(AtomicBool::new(false));
        let slow = Arc::new(AtomicBool::new(false));
        let rotated = Arc::new(AtomicBool::new(false));
        let response_body = Arc::new(std::sync::Mutex::new(None::<String>));
        let revoked = Arc::new(AtomicBool::new(false));
        let revoke_flag = revoked.clone();
        let (count, fail) = (calls.clone(), outage.clone());
        let (delay, rotation) = (slow.clone(), rotated.clone());
        let reply = response_body.clone();
        let app = Router::new().route(
            "/.well-known/jwks.json",
            get(move || {
                let (count, fail) = (count.clone(), fail.clone());
                let (delay, rotation) = (delay.clone(), rotation.clone());
                let reply = reply.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    if delay.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_secs(4)).await;
                    }
                    if fail.load(Ordering::SeqCst) {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    if let Some(body) = reply.lock().unwrap().clone() {
                        return ([(header::CONTENT_TYPE, "application/json")], body)
                            .into_response();
                    }
                    let mut jwks = serde_json::from_str::<Value>(include_str!(
                        "../../tests/fixtures/auth-test-jwks.json"
                    ))
                    .unwrap();
                    if rotation.load(Ordering::SeqCst) {
                        jwks["keys"][0]["kid"] = json!("rotated");
                    }
                    Json(jwks).into_response()
                }
            }),
        );
        let app = app.route(
            "/v1/auth/check",
            axum::routing::post(move || {
                let flag = revoke_flag.clone();
                async move {
                    if flag.load(Ordering::SeqCst) {
                        StatusCode::UNAUTHORIZED
                    } else {
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let auth = Arc::new(AuthState::new(AuthConfig {
            issuer_url: format!("http://{}", listener.local_addr().unwrap()),
            client_id: "fixture-client".into(),
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            auth,
            calls,
            outage,
            slow,
            rotated,
            response_body,
            revoked,
            server,
        }
    }
    fn claims(&self) -> Value {
        let now = chrono::Utc::now().timestamp();
        json!({"iss": self.auth.config.issuer_url, "sub":"fixture-user", "client_id":"fixture-client", "token_use":"access", "iat":now, "auth_time":now, "exp":now+300})
    }
    fn token(&self, claims: &Value) -> String {
        self.token_with_kid(claims, "fixture")
    }
    fn token_with_kid(&self, claims: &Value, kid: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.into());
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(include_bytes!(
                "../../tests/fixtures/auth-test-private.pem"
            ))
            .unwrap(),
        )
        .unwrap()
    }

    async fn wait_for_refresh(&self, expected_calls: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if self.calls.load(Ordering::SeqCst) >= expected_calls {
                if let Ok(last) = self.auth.keys.last_refresh.try_lock() {
                    if last.as_ref().is_some_and(|attempt| {
                        attempt.succeeded == !self.outage.load(Ordering::SeqCst)
                    }) {
                        return;
                    }
                }
            }
            tokio::task::yield_now().await;
        }
        panic!(
            "scheduled refresh did not finish: expected {expected_calls} calls, observed {}; task finished: {}",
            self.calls.load(Ordering::SeqCst),
            self.auth.refresh_task.is_finished(),
        );
    }

    async fn wait_for_request(&self, expected_calls: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if self.calls.load(Ordering::SeqCst) >= expected_calls {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("issuer did not receive {expected_calls} requests");
    }
}

#[tokio::test]
async fn http_middleware_checks_current_authority_after_jwt_validation() {
    use axum::{body::Body, middleware};
    use tower::ServiceExt;
    let f = Fixture::new().await;
    let previous_url = std::env::var_os("SULION_SECRET_BROKER_URL");
    let previous_token = std::env::var_os("SULION_SECRET_BROKER_REGISTRATION_TOKEN");
    std::env::set_var("SULION_SECRET_BROKER_URL", &f.auth.config.issuer_url);
    std::env::set_var(
        "SULION_SECRET_BROKER_REGISTRATION_TOKEN",
        "test-fixture-only",
    );
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://fixture:fixture@localhost/unused")
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let state = AppState::new_with_auth(
        pool,
        root.path().into(),
        root.path().into(),
        root.path().into(),
        Arc::new(crate::ingest::Ingester::new()),
        Some(f.auth.clone()),
    );
    let app = Router::new()
        .route("/probe", get(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state(state, require_http_auth));
    let token = f.token(&f.claims());
    for (revoked, expected) in [(false, StatusCode::OK), (true, StatusCode::UNAUTHORIZED)] {
        f.revoked.store(revoked, Ordering::SeqCst);
        let request = axum::http::Request::builder()
            .uri("/probe")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            expected
        );
    }
    f.revoked.store(false, Ordering::SeqCst);
    f.outage.store(true, Ordering::SeqCst);
    f.auth
        .keys
        .jwks_cache
        .write()
        .await
        .as_mut()
        .unwrap()
        .loaded_at = Instant::now() - JWKS_CACHE_TTL;
    f.auth.keys.last_refresh.lock().await.as_mut().unwrap().at -= JWKS_REFRESH_COOLDOWN;
    let request = axum::http::Request::builder()
        .uri("/probe")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::RETRY_AFTER], "30");
    for (key, value) in [
        ("SULION_SECRET_BROKER_URL", previous_url),
        ("SULION_SECRET_BROKER_REGISTRATION_TOKEN", previous_token),
    ] {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}

#[tokio::test]
async fn signed_claims_are_required_and_verified() {
    let f = Fixture::new().await;
    let good = f.claims();
    assert_eq!(
        f.auth.validate_bearer(&f.token(&good)).await.unwrap().sub,
        "fixture-user"
    );
    for (claim, bad) in [
        ("iss", json!("https://wrong")),
        ("client_id", json!("wrong")),
        ("token_use", json!("id")),
        ("exp", json!(1)),
        ("sub", json!("")),
        ("auth_time", json!(i64::MAX)),
    ] {
        let mut claims = good.clone();
        claims[claim] = bad;
        assert!(
            f.auth.validate_bearer(&f.token(&claims)).await.is_err(),
            "{claim}"
        );
    }
    for claim in [
        "iss",
        "sub",
        "exp",
        "iat",
        "auth_time",
        "token_use",
        "client_id",
    ] {
        let mut claims = good.clone();
        claims.as_object_mut().unwrap().remove(claim);
        assert!(
            f.auth.validate_bearer(&f.token(&claims)).await.is_err(),
            "missing {claim}"
        );
    }
    let token = f.token(&good);
    let (body, signature) = token.rsplit_once('.').unwrap();
    let mut signature = signature.as_bytes().to_vec();
    signature[0] = if signature[0] == b'A' { b'B' } else { b'A' };
    assert!(f
        .auth
        .validate_bearer(&format!("{body}.{}", String::from_utf8(signature).unwrap()))
        .await
        .is_err());
}

#[tokio::test]
async fn unknown_keys_coalesce_refresh_and_rotation_respects_cooldown() {
    let f = Fixture::new().await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let auth = f.auth.clone();
        tasks.spawn(async move {
            let error = auth.keys.find_key("unknown").await.err().unwrap();
            assert_eq!(
                authentication_error_response(&error).status(),
                StatusCode::UNAUTHORIZED
            );
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    assert!(f.auth.keys.find_key("fixture").await.is_ok());
    // Rotation can refresh after cooldown, even when the old cache is fresh.
    f.rotated.store(true, Ordering::SeqCst);
    assert!(f.auth.keys.find_key("rotated").await.is_err());
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    f.auth.keys.last_refresh.lock().await.as_mut().unwrap().at -= JWKS_REFRESH_COOLDOWN;
    let token = f.token_with_kid(&f.claims(), "rotated");
    assert!(f.auth.validate_bearer(&token).await.is_ok());
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    assert!(f.auth.validate_bearer(&f.token(&f.claims())).await.is_err());
}

#[tokio::test]
async fn algorithm_rejection_precedes_network_and_outage_is_bounded() {
    let f = Fixture::new().await;
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("fixture".into());
    let token = encode(&header, &f.claims(), &EncodingKey::from_secret(b"fixture")).unwrap();
    assert!(f.auth.validate_bearer(&token).await.is_err());
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    f.slow.store(true, Ordering::SeqCst);
    let start = Instant::now();
    let error = f.auth.keys.find_key("fixture").await.err().unwrap();
    assert_eq!(
        authentication_error_response(&error).status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(start.elapsed() < Duration::from_secs(4));
    let error = f.auth.keys.find_key("fixture").await.err().unwrap();
    assert_eq!(
        authentication_error_response(&error).status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn scheduled_refresh_survives_three_failures_then_expires_and_recovers() {
    let f = Fixture::new().await;
    assert!(f.auth.validate_bearer(&f.token(&f.claims())).await.is_ok());
    tokio::task::yield_now().await;
    tokio::time::pause();
    f.outage.store(true, Ordering::SeqCst);
    for attempt in 1..=3 {
        tokio::time::advance(JWKS_REFRESH_INTERVAL + Duration::from_millis(1)).await;
        f.wait_for_refresh(attempt + 1).await;
        assert!(f.auth.validate_bearer(&f.token(&f.claims())).await.is_ok());
        assert_eq!(f.calls.load(Ordering::SeqCst), attempt + 1);
    }
    tokio::time::advance(JWKS_REFRESH_INTERVAL + Duration::from_millis(1)).await;
    f.wait_for_refresh(5).await;
    let error = f
        .auth
        .validate_bearer(&f.token(&f.claims()))
        .await
        .err()
        .unwrap();
    assert_eq!(
        authentication_error_response(&error).status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(f.calls.load(Ordering::SeqCst), 5);

    f.outage.store(false, Ordering::SeqCst);
    tokio::time::advance(JWKS_REFRESH_COOLDOWN).await;
    let auth = f.auth.clone();
    let token = f.token(&f.claims());
    let recovery = tokio::spawn(async move { auth.validate_bearer(&token).await });
    f.wait_for_refresh(6).await;
    assert!(recovery.await.unwrap().is_ok());
    assert_eq!(f.calls.load(Ordering::SeqCst), 6);
    // A successful fetch starts a new 60-minute validity window.
    tokio::time::advance(JWKS_REFRESH_INTERVAL + Duration::from_millis(1)).await;
    f.wait_for_refresh(7).await;
    assert!(f.auth.validate_bearer(&f.token(&f.claims())).await.is_ok());
}

#[tokio::test]
async fn successful_scheduled_refresh_replaces_keys_without_a_browser_request() {
    let f = Fixture::new().await;
    assert!(f.auth.validate_bearer(&f.token(&f.claims())).await.is_ok());
    tokio::task::yield_now().await;
    tokio::time::pause();
    f.rotated.store(true, Ordering::SeqCst);
    tokio::time::advance(JWKS_REFRESH_INTERVAL + Duration::from_millis(1)).await;
    f.wait_for_refresh(2).await;
    let token = f.token_with_kid(&f.claims(), "rotated");
    assert!(f.auth.validate_bearer(&token).await.is_ok());
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cached_authentication_does_not_wait_for_a_slow_background_refresh() {
    let f = Fixture::new().await;
    let token = f.token(&f.claims());
    assert!(f.auth.validate_bearer(&token).await.is_ok());
    tokio::task::yield_now().await;
    tokio::time::pause();
    f.slow.store(true, Ordering::SeqCst);
    tokio::time::advance(JWKS_REFRESH_INTERVAL + Duration::from_millis(1)).await;
    f.wait_for_request(2).await;
    assert!(
        f.auth.keys.last_refresh.try_lock().is_err(),
        "refresh is still in flight"
    );
    let user = tokio::time::timeout(Duration::from_millis(100), f.auth.validate_bearer(&token))
        .await
        .expect("known signing keys must remain available during a refresh")
        .unwrap();
    assert_eq!(user.sub, "fixture-user");
}

#[tokio::test]
async fn malformed_refresh_responses_preserve_keys_without_extending_their_lifetime() {
    for body in [
        "not JSON",
        r#"{"keys":[]}"#,
        r#"{"keys":[{"kid":"fixture","kty":"RSA","n":"!","e":"AQAB"}]}"#,
    ] {
        let f = Fixture::new().await;
        let token = f.token(&f.claims());
        assert!(f.auth.validate_bearer(&token).await.is_ok());
        let loaded_at = f
            .auth
            .keys
            .jwks_cache
            .read()
            .await
            .as_ref()
            .unwrap()
            .loaded_at;
        *f.response_body.lock().unwrap() = Some(body.into());
        let error = f
            .auth
            .keys
            .refresh_jwks(&mut *f.auth.keys.last_refresh.lock().await)
            .await
            .err()
            .expect("invalid key sets must not replace cached keys");
        assert!(!error.to_string().is_empty());
        assert_eq!(
            f.auth
                .keys
                .jwks_cache
                .read()
                .await
                .as_ref()
                .unwrap()
                .loaded_at,
            loaded_at
        );
        assert!(f.auth.validate_bearer(&token).await.is_ok());
        f.auth
            .keys
            .jwks_cache
            .write()
            .await
            .as_mut()
            .unwrap()
            .loaded_at = Instant::now() - JWKS_CACHE_TTL;
        let error = f.auth.validate_bearer(&token).await.err().unwrap();
        assert_eq!(
            authentication_error_response(&error).status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            f.calls.load(Ordering::SeqCst),
            2,
            "failed refresh cooldown must coalesce retries"
        );
    }
}

#[tokio::test]
async fn concurrent_expired_cache_requests_coalesce_failure_and_recovery() {
    let f = Fixture::new().await;
    let token = f.token(&f.claims());
    assert!(f.auth.validate_bearer(&token).await.is_ok());
    f.outage.store(true, Ordering::SeqCst);
    f.auth
        .keys
        .jwks_cache
        .write()
        .await
        .as_mut()
        .unwrap()
        .loaded_at = Instant::now() - JWKS_CACHE_TTL;
    f.auth.keys.last_refresh.lock().await.as_mut().unwrap().at -= JWKS_REFRESH_COOLDOWN;
    for available in [false, true] {
        f.outage.store(!available, Ordering::SeqCst);
        if available {
            f.auth.keys.last_refresh.lock().await.as_mut().unwrap().at -= JWKS_REFRESH_COOLDOWN;
        }
        let mut requests = tokio::task::JoinSet::new();
        for _ in 0..32 {
            let auth = f.auth.clone();
            let token = token.clone();
            requests.spawn(async move {
                match auth.validate_bearer(&token).await {
                    Ok(user) => {
                        assert!(available);
                        assert_eq!(user.sub, "fixture-user");
                    }
                    Err(error) => {
                        assert!(!available);
                        assert_eq!(
                            authentication_error_response(&error).status(),
                            StatusCode::SERVICE_UNAVAILABLE
                        );
                    }
                }
            });
        }
        while let Some(result) = requests.join_next().await {
            result.unwrap();
        }
        assert_eq!(
            f.calls.load(Ordering::SeqCst),
            if available { 3 } else { 2 }
        );
    }
}

#[tokio::test]
async fn cached_keys_do_not_relax_token_validation_during_an_outage() {
    let f = Fixture::new().await;
    let good = f.claims();
    assert!(f.auth.validate_bearer(&f.token(&good)).await.is_ok());
    f.outage.store(true, Ordering::SeqCst);
    f.auth
        .keys
        .jwks_cache
        .write()
        .await
        .as_mut()
        .unwrap()
        .loaded_at = Instant::now() - JWKS_REFRESH_INTERVAL * 3;
    for (claim, value) in [
        ("exp", json!(1)),
        ("client_id", json!("another-client")),
        ("iss", json!("https://another-issuer")),
        ("token_use", json!("id")),
    ] {
        let mut claims = good.clone();
        claims[claim] = value;
        let error = f
            .auth
            .validate_bearer(&f.token(&claims))
            .await
            .err()
            .unwrap();
        assert_eq!(
            authentication_error_response(&error).status(),
            StatusCode::UNAUTHORIZED,
            "{claim}"
        );
    }
    let unknown = f.token_with_kid(&good, "unknown");
    f.auth.keys.last_refresh.lock().await.as_mut().unwrap().at -= JWKS_REFRESH_COOLDOWN;
    let error = f.auth.validate_bearer(&unknown).await.err().unwrap();
    assert_eq!(
        authentication_error_response(&error).status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(f.auth.validate_bearer(&f.token(&good)).await.is_ok());
}

#[tokio::test]
async fn cached_keys_are_usable_until_the_exact_sixty_minute_boundary() {
    let f = Fixture::new().await;
    let token = f.token(&f.claims());
    assert!(f.auth.validate_bearer(&token).await.is_ok());
    tokio::time::pause();
    f.outage.store(true, Ordering::SeqCst);
    f.auth
        .keys
        .jwks_cache
        .write()
        .await
        .as_mut()
        .unwrap()
        .loaded_at = Instant::now() - JWKS_CACHE_TTL + Duration::from_millis(1);
    assert!(f.auth.validate_bearer(&token).await.is_ok());
    tokio::time::advance(Duration::from_millis(1)).await;
    let error = f.auth.validate_bearer(&token).await.err().unwrap();
    assert_eq!(
        authentication_error_response(&error).status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        f.calls.load(Ordering::SeqCst),
        1,
        "cache expiry must respect the existing fetch cooldown"
    );
}

#[tokio::test]
async fn dropping_authentication_state_stops_periodic_refreshes() {
    let f = Fixture::new().await;
    assert!(f.auth.validate_bearer(&f.token(&f.claims())).await.is_ok());
    let task = f.auth.refresh_task.abort_handle();
    let calls = f.calls.clone();
    tokio::task::yield_now().await;
    tokio::time::pause();
    drop(f);
    tokio::task::yield_now().await;
    assert!(task.is_finished());
    tokio::time::advance(JWKS_REFRESH_INTERVAL * 2).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
