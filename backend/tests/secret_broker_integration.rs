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

    fn signed(&self) -> Value {
        self.signed_for(OTHER_PROGRAM)
    }

    fn signed_for(&self, program: &str) -> Value {
        let timestamp = chrono::Utc::now().timestamp();
        let nonce = Uuid::new_v4().to_string();
        let payload = canonical_use_payload(self.id, program, timestamp, &nonce);
        json!({"pty_session_id": self.id, "program": program,
            "timestamp_unix_seconds": timestamp, "nonce": nonce,
            "signature": BASE64_STANDARD.encode(self.key.sign(payload.as_bytes()).as_ref())})
    }

    /// Redeem for a program no all-terminals grant lists.
    async fn redeem(&self, app: &Router) -> (StatusCode, Value) {
        self.run(app, OTHER_PROGRAM).await
    }

    async fn run(&self, app: &Router, program: &str) -> (StatusCode, Value) {
        request(
            app.clone(),
            "POST",
            "/v1/use",
            self.signed_for(program),
            false,
        )
        .await
    }
}

const GH: &str = "gh";
const OTHER_PROGRAM: &str = "make";

/// A broker on its own database. Management routes run without browser
/// authentication through `management_app_for_tests`.
struct TestBroker {
    app: Router,
    management: Router,
    admin: sulion::db::Pool,
    database: String,
    _tmp: tempfile::TempDir,
}

impl TestBroker {
    async fn start() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let key = tmp.path().join("master.key");
        tokio::fs::write(&key, [7_u8; 32]).await.unwrap();
        let test_url = std::env::var("SULION_TEST_DB").expect("run through integration harness");
        let admin = sulion::db::connect(&test_url).await.unwrap();
        let database = format!("broker_test_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {database}"))
            .execute(&admin)
            .await
            .unwrap();
        let mut broker_url = url::Url::parse(&test_url).unwrap();
        broker_url.set_path(&format!("/{database}"));
        let state = BrokerState::from_config(&BrokerConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            db_url: broker_url.to_string(),
            master_key_path: key,
            auth_issuer_url: "http://127.0.0.1:9".into(),
            auth_client_id: "test".into(),
            registration_token: "test-registration".into(),
        })
        .await
        .unwrap();
        Self {
            app: secret_broker::app(state.clone()),
            management: secret_broker::management_app_for_tests(state),
            admin,
            database,
            _tmp: tmp,
        }
    }

    async fn put_secret(&self, id: &str, env: Value) {
        let body = json!({"description": id, "scope": "global", "repo": null, "env": env});
        let path = format!("/v1/secrets/{id}");
        let (status, _) = request(self.management.clone(), "PUT", &path, body, false).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    async fn manage(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        request(self.management.clone(), method, path, body, false).await
    }

    async fn drop(self) {
        sqlx::query(&format!("DROP DATABASE {} WITH (FORCE)", self.database))
            .execute(&self.admin)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn all_terminals_grant_applies_to_listed_programs_below_explicit_grants() {
    let broker = TestBroker::start().await;
    broker
        .put_secret("gh-read", json!({"GH_TOKEN": "read-token"}))
        .await;
    broker
        .put_secret(
            "gh-write",
            json!({"GH_TOKEN": "write-token", "GH_HOST": "github.com"}),
        )
        .await;
    broker
        .put_secret("gh-other", json!({"GH_TOKEN": "other-token"}))
        .await;
    let a = Pty::register(&broker.app, Some("atlas")).await;
    // A credential without a repository, as the node's CI poller registers.
    let b = Pty::register(&broker.app, None).await;

    assert_eq!(a.redeem(&broker.app).await.0, StatusCode::FORBIDDEN);

    let every =
        |secret: &str| json!({"secret_id": secret, "scope": "all_terminals", "programs": [GH]});
    for invalid in [
        json!({"secret_id": "gh-read", "scope": "all_terminals"}),
        json!({"secret_id": "gh-read", "scope": "all_terminals", "programs": []}),
        json!({"secret_id": "gh-read", "scope": "all_terminals", "programs": ["/usr/bin/gh"]}),
        json!({"secret_id": "gh-read", "scope": "all_terminals", "programs": [GH], "ttl_seconds": 600}),
        json!({"pty_session_id": a.id, "secret_id": "gh-read", "ttl_seconds": 600, "programs": [GH]}),
    ] {
        let (status, _) = broker.manage("POST", "/v1/grants", invalid.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid}");
    }
    for _ in 0..2 {
        let (status, _) = broker.manage("POST", "/v1/grants", every("gh-read")).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "granting again replaces the grant"
        );
    }
    let (_, secrets) = broker.manage("GET", "/v1/secrets", Value::Null).await;
    let programs = |id: &str| {
        secrets
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .unwrap()["all_terminal_programs"]
            .clone()
    };
    assert_eq!(programs("gh-read"), json!([GH]));
    assert_eq!(programs("gh-write"), Value::Null);

    // Every terminal, with or without a repository, gets the read token for
    // the listed program, and only for it.
    let future = Pty::register(&broker.app, Some("elsewhere")).await;
    for pty in [&a, &b, &future] {
        let (status, body) = pty.run(&broker.app, GH).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["env"]["GH_TOKEN"], "read-token");
        assert_eq!(
            pty.redeem(&broker.app).await.0,
            StatusCode::FORBIDDEN,
            "any other program still needs a grant"
        );
    }
    let (_, grants) = broker
        .manage(
            "GET",
            &format!("/v1/grants?pty_session_id={}", a.id),
            Value::Null,
        )
        .await;
    assert_eq!(grants[0]["scope"], "all_terminals");
    assert_eq!(grants[0]["programs"], json!([GH]));

    // An explicit grant supersedes it, for that terminal only, and applies to
    // every program.
    let write = json!({"pty_session_id": a.id, "secret_id": "gh-write", "ttl_seconds": 600});
    assert_eq!(
        broker.manage("POST", "/v1/grants", write).await.0,
        StatusCode::CREATED
    );
    for program in [GH, OTHER_PROGRAM] {
        let (status, body) = a.run(&broker.app, program).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["env"]["GH_TOKEN"], "write-token");
        assert_eq!(body["env"]["GH_HOST"], "github.com");
    }
    assert_eq!(
        b.run(&broker.app, GH).await.1["env"]["GH_TOKEN"],
        "read-token"
    );

    // Two all-terminals secrets setting one variable is an explicit conflict.
    let (status, _) = broker.manage("POST", "/v1/grants", every("gh-other")).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(b.run(&broker.app, GH).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        a.run(&broker.app, GH).await.1["env"]["GH_TOKEN"],
        "write-token",
        "an explicit grant still outranks both"
    );
    let revoke = |secret: &str| json!({"secret_id": secret, "scope": "all_terminals"});
    for secret in ["gh-other", "gh-read"] {
        let (status, _) = broker.manage("DELETE", "/v1/grants", revoke(secret)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    assert_eq!(b.run(&broker.app, GH).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        a.redeem(&broker.app).await.1["env"]["GH_TOKEN"],
        "write-token"
    );
    broker.drop().await;
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
    assert_eq!(a.redeem(&app).await.0, StatusCode::FORBIDDEN);
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
        let (status, body) = pty.redeem(&restarted).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["env"]["STATUS_TOKEN"], "test-value");
    }
    assert_eq!(b.redeem(&restarted).await.0, StatusCode::FORBIDDEN);
    assert_eq!(legacy.redeem(&restarted).await.0, StatusCode::FORBIDDEN);
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
    assert_eq!(legacy.redeem(&restarted).await.0, StatusCode::OK);
    let revoke_path = format!("/v1/pty-credentials/{}", legacy.id);
    assert_eq!(
        request(restarted.clone(), "DELETE", &revoke_path, Value::Null, true)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(legacy.redeem(&restarted).await.0, StatusCode::UNAUTHORIZED);
    let mut forged_repo = b.signed();
    forged_repo["repo"] = json!("atlas");
    assert_eq!(
        request(restarted.clone(), "POST", "/v1/use", forged_repo, false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let mut invalid_signature = a.signed();
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
    let signed = a.signed();
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
        a.redeem(&restarted).await.0,
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
    assert_eq!(a.redeem(&restarted).await.0, StatusCode::BAD_REQUEST);
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
    assert_eq!(future.redeem(&restarted).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        a.redeem(&restarted).await.0,
        StatusCode::OK,
        "terminal grant remains"
    );
    sqlx::query("UPDATE secret_broker.grants SET expires_at = NOW() - INTERVAL '1 second' WHERE pty_session_id = $1").bind(a.id).execute(&state.pool).await.unwrap();
    assert_eq!(a.redeem(&restarted).await.0, StatusCode::FORBIDDEN);
    let (_, envelope) = request(management, "GET", "/v1/secrets/status", Value::Null, false).await;
    assert_eq!(envelope["env"]["STATUS_TOKEN"], "");
    sqlx::query(&format!("DROP DATABASE {database} WITH (FORCE)"))
        .execute(&admin)
        .await
        .unwrap();
}
