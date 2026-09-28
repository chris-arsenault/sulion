use super::*;
use axum::{routing::get, Json, Router};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Fixture {
    auth: Arc<AuthState>,
    calls: Arc<AtomicUsize>,
    outage: Arc<AtomicBool>,
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
        let revoked = Arc::new(AtomicBool::new(false));
        let revoke_flag = revoked.clone();
        let (count, fail) = (calls.clone(), outage.clone());
        let app = Router::new().route(
            "/.well-known/jwks.json",
            get(move || {
                let (count, fail) = (count.clone(), fail.clone());
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    if fail.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_secs(4)).await;
                    }
                    Json(
                        serde_json::from_str::<Value>(include_str!(
                            "../../tests/fixtures/auth-test-jwks.json"
                        ))
                        .unwrap(),
                    )
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
            revoked,
            server,
        }
    }
    fn claims(&self) -> Value {
        let now = chrono::Utc::now().timestamp();
        json!({"iss": self.auth.config.issuer_url, "sub":"fixture-user", "client_id":"fixture-client", "token_use":"access", "iat":now, "auth_time":now, "exp":now+300})
    }
    fn token(&self, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("fixture".into());
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
async fn refresh_is_coalesced_cooled_down_and_expires() {
    let f = Fixture::new().await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let auth = f.auth.clone();
        tasks.spawn(async move {
            assert!(auth.find_key("unknown").await.is_err());
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    assert!(f.auth.find_key("fixture").await.is_ok());
    // Rotation can refresh after cooldown, even when the old cache is fresh.
    f.auth
        .jwks_cache
        .write()
        .await
        .as_mut()
        .unwrap()
        .keys
        .clear();
    *f.auth.last_refresh.lock().await = Some(Instant::now() - JWKS_REFRESH_COOLDOWN);
    assert!(f.auth.find_key("fixture").await.is_ok());
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    f.auth.jwks_cache.write().await.as_mut().unwrap().loaded_at = Instant::now() - JWKS_CACHE_TTL;
    assert!(f.auth.find_key("fixture").await.is_err());
}

#[tokio::test]
async fn algorithm_rejection_precedes_network_and_outage_is_bounded() {
    let f = Fixture::new().await;
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("fixture".into());
    let token = encode(&header, &f.claims(), &EncodingKey::from_secret(b"fixture")).unwrap();
    assert!(f.auth.validate_bearer(&token).await.is_err());
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    f.outage.store(true, Ordering::SeqCst);
    let start = Instant::now();
    assert!(f.auth.find_key("fixture").await.is_err());
    assert!(start.elapsed() < Duration::from_secs(4));
    assert!(f.auth.find_key("fixture").await.is_err());
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
}
