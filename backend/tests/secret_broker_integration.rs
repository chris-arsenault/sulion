#![cfg(feature = "integration-tests")]

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use base64::prelude::*;
use ring::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};
use serde_json::{json, Value};
use sulion::{
    secret_broker::{self, BrokerConfig, BrokerState},
    secret_protocol::canonical_use_payload,
};
use tower::ServiceExt;
use uuid::Uuid;

async fn request(
    app: Router,
    method: &str,
    path: &str,
    body: Value,
    registration: bool,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if registration {
        builder = builder.header("authorization", "Bearer test-registration");
    }
    let response = app
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

struct Pty {
    id: Uuid,
    key: Ed25519KeyPair,
}

impl Pty {
    async fn register(app: &Router, repo: Option<&str>) -> Self {
        let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        let pty = Self {
            id: Uuid::new_v4(),
            key: Ed25519KeyPair::from_pkcs8(key.as_ref()).unwrap(),
        };
        let (status, _) = request(app.clone(), "POST", "/v1/pty-credentials", json!({
            "pty_session_id": pty.id, "public_key": BASE64_STANDARD.encode(pty.key.public_key().as_ref()), "repo": repo,
        }), true).await;
        assert_eq!(status, StatusCode::CREATED);
        pty
    }

    fn signed(&self, secret: Option<&str>) -> Value {
        let timestamp = chrono::Utc::now().timestamp();
        let nonce = Uuid::new_v4().to_string();
        let payload = canonical_use_payload(self.id, secret, "with-cred", timestamp, &nonce);
        json!({"pty_session_id": self.id, "secret_id": secret, "tool": "with-cred",
            "timestamp_unix_seconds": timestamp, "nonce": nonce,
            "signature": BASE64_STANDARD.encode(self.key.sign(payload.as_bytes()).as_ref())})
    }

    async fn redeem(&self, app: &Router, secret: Option<&str>) -> (StatusCode, Value) {
        request(app.clone(), "POST", "/v1/use", self.signed(secret), false).await
    }
}

#[tokio::test]
async fn repository_grants_persist_apply_to_future_sessions_and_revoke_independently() {
    let tmp = tempfile::tempdir().unwrap();
    let key = tmp.path().join("master.key");
    tokio::fs::write(&key, [42_u8; 32]).await.unwrap();
    // Broker migrations have their own ledger in production's separate database.
    let test_url = std::env::var("SULION_TEST_DB").expect("run through integration harness");
    let admin = sulion::db::connect(&test_url).await.unwrap();
    let database = format!("broker_test_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {database}"))
        .execute(&admin)
        .await
        .unwrap();
    let mut broker_url = url::Url::parse(&test_url).unwrap();
    broker_url.set_path(&format!("/{database}"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let jwks = axum::Router::new().route(
        "/.well-known/jwks.json",
        axum::routing::get(|| async {
            axum::Json(
                serde_json::from_str::<Value>(include_str!("fixtures/auth-test-jwks.json"))
                    .unwrap(),
            )
        }),
    );
    let issuer_server = tokio::spawn(async move {
        axum::serve(listener, jwks).await.unwrap();
    });
    let config = BrokerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        db_url: broker_url.to_string(),
        master_key_path: key,
        auth_issuer_url: issuer.clone(),
        auth_client_id: "test".into(),
        registration_token: "test-registration".into(),
    };
    let state = BrokerState::from_config(&config).await.unwrap();
    sqlx::query("TRUNCATE secret_broker.secrets, secret_broker.pty_credentials CASCADE")
        .execute(&state.pool)
        .await
        .unwrap();
    let app = secret_broker::app(state.clone());
    let management = secret_broker::management_app_for_tests(state.clone());
    let now = chrono::Utc::now().timestamp();
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("fixture".into());
    let token = jsonwebtoken::encode(
        &header,
        &json!({
            "iss":issuer,"sub":"browser-fixture","client_id":"test","token_use":"access",
            "iat":now,"auth_time":now-10,"exp":now+300
        }),
        &jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("fixtures/auth-test-private.pem"))
            .unwrap(),
    )
    .unwrap();
    for (method, path, expected) in [
        ("GET", "/v1/secrets", StatusCode::OK),
        ("POST", "/v1/session/revoke", StatusCode::NO_CONTENT),
        ("GET", "/v1/secrets", StatusCode::UNAUTHORIZED),
        ("POST", "/v1/session/revoke", StatusCode::NO_CONTENT),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{method} {path}");
    }
    issuer_server.abort();
    let now = chrono::Utc::now().timestamp();
    let authority = json!({"sub":"controlled-test-user", "auth_time":now-10, "expires_at":now+300});
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/v1/auth/check",
            authority.clone(),
            true
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/v1/auth/revoke",
            json!({"sub":"controlled-test-user"}),
            false
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/v1/auth/revoke",
            json!({"sub":"controlled-test-user"}),
            true
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    // A new access token obtained by refreshing the old login retains auth_time.
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/v1/auth/check",
            authority.clone(),
            true
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let restarted = BrokerState::from_config(&config).await.unwrap();
    assert_eq!(
        request(
            secret_broker::app(restarted),
            "POST",
            "/v1/auth/check",
            authority,
            true
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/v1/pty-credentials?access_token=test-registration",
            json!({}),
            false
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let a = Pty::register(&app, Some("atlas")).await;
    let b = Pty::register(&app, Some("other")).await;
    let legacy = Pty::register(&app, None).await;
    let secret = json!({"description":"status", "scope":"global", "repo":null, "env":{"STATUS_TOKEN":"test-value"}});
    assert_eq!(
        request(
            management.clone(),
            "PUT",
            "/v1/secrets/status",
            secret,
            false
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let grant = json!({"pty_session_id":a.id, "secret_id":"status", "scope":"repository"});
    assert_eq!(
        request(app.clone(), "POST", "/v1/grants", grant.clone(), false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(a.redeem(&app, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        request(
            management.clone(),
            "POST",
            "/v1/grants",
            json!({"pty_session_id":legacy.id,"secret_id":"status","scope":"repository"}),
            false
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(request(management.clone(), "POST", "/v1/grants", json!({"pty_session_id":a.id,"secret_id":"status","scope":"repository","ttl_seconds":600}), false).await.0, StatusCode::BAD_REQUEST);
    for _ in 0..2 {
        assert_eq!(
            request(
                management.clone(),
                "POST",
                "/v1/grants",
                grant.clone(),
                false
            )
            .await
            .0,
            StatusCode::CREATED
        );
    }
    // Reconstruct state against persisted data before a new session registers.
    let restarted = secret_broker::app(BrokerState::from_config(&config).await.unwrap());
    let future = Pty::register(&restarted, Some("atlas")).await;
    for pty in [&a, &future] {
        let (status, body) = pty.redeem(&restarted, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["env"]["STATUS_TOKEN"], "test-value");
    }
    assert_eq!(
        b.redeem(&restarted, Some("status")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        legacy.redeem(&restarted, None).await.0,
        StatusCode::FORBIDDEN
    );
    let adopted = json!({
        "pty_session_id": legacy.id,
        "public_key": BASE64_STANDARD.encode(legacy.key.public_key().as_ref()),
        "repo": "atlas",
    });
    assert_eq!(
        request(
            restarted.clone(),
            "POST",
            "/v1/pty-credentials",
            adopted,
            true
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(legacy.redeem(&restarted, None).await.0, StatusCode::OK);
    let revoke_path = format!("/v1/pty-credentials/{}", legacy.id);
    assert_eq!(
        request(restarted.clone(), "DELETE", &revoke_path, Value::Null, true)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        legacy.redeem(&restarted, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let mut forged_repo = b.signed(Some("status"));
    forged_repo["repo"] = json!("atlas");
    assert_eq!(
        request(restarted.clone(), "POST", "/v1/use", forged_repo, false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let mut invalid_signature = a.signed(Some("status"));
    invalid_signature["signature"] = json!(BASE64_STANDARD.encode([0_u8; 64]));
    assert_eq!(
        request(
            restarted.clone(),
            "POST",
            "/v1/use",
            invalid_signature,
            false
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let signed = a.signed(Some("status"));
    assert_eq!(
        request(restarted.clone(), "POST", "/v1/use", signed.clone(), false)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(restarted.clone(), "POST", "/v1/use", signed, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let timed = json!({"pty_session_id":a.id,"secret_id":"status","ttl_seconds":600});
    assert_eq!(
        request(management.clone(), "POST", "/v1/grants", timed, false)
            .await
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        a.redeem(&restarted, None).await.0,
        StatusCode::OK,
        "same secret in two scopes is merged once"
    );
    let (_, grants) = request(
        management.clone(),
        "GET",
        &format!("/v1/grants?pty_session_id={}", a.id),
        Value::Null,
        false,
    )
    .await;
    assert_eq!(grants.as_array().unwrap().len(), 2);
    assert!(grants
        .as_array()
        .unwrap()
        .iter()
        .any(|g| g["repo"] == "atlas" && g["expires_at"].is_null()));
    let conflict = json!({"description":"conflict", "scope":"global", "repo":null, "env":{"STATUS_TOKEN":"different"}});
    assert_eq!(
        request(
            management.clone(),
            "PUT",
            "/v1/secrets/conflict",
            conflict,
            false
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let conflict_grant = json!({"pty_session_id":a.id,"secret_id":"conflict","ttl_seconds":600});
    assert_eq!(
        request(
            management.clone(),
            "POST",
            "/v1/grants",
            conflict_grant,
            false
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(a.redeem(&restarted, None).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(a.redeem(&restarted, Some("status")).await.0, StatusCode::OK);
    assert_eq!(
        request(
            management.clone(),
            "DELETE",
            "/v1/secrets/conflict",
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(management.clone(), "DELETE", "/v1/grants", grant, false)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        future.redeem(&restarted, None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        a.redeem(&restarted, None).await.0,
        StatusCode::OK,
        "terminal grant remains"
    );
    sqlx::query("UPDATE secret_broker.grants SET expires_at = NOW() - INTERVAL '1 second' WHERE pty_session_id = $1").bind(a.id).execute(&state.pool).await.unwrap();
    assert_eq!(a.redeem(&restarted, None).await.0, StatusCode::FORBIDDEN);
    let (_, envelope) = request(management, "GET", "/v1/secrets/status", Value::Null, false).await;
    assert_eq!(envelope["env"]["STATUS_TOKEN"], "");
    sqlx::query(&format!("DROP DATABASE {database} WITH (FORCE)"))
        .execute(&admin)
        .await
        .unwrap();
}
