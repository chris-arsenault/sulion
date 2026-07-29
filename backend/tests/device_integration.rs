#![cfg(feature = "integration-tests")]

//! Device-pairing + repo content-ingest integration tests: full axum stack,
//! real Postgres, real filesystem. Auth is disabled (the harness uses no
//! `AuthState`), so the approval handler binds to the synthetic `dev`
//! principal. Gated on `SULION_TEST_DB`.

use std::path::PathBuf;

use serde_json::json;
use sulion::{app, db};
use tokio::net::TcpListener;

mod common;

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    db::run_migrations(&pool).await.expect("migrate");
    sqlx::query("TRUNCATE device_pairings, device_tokens RESTART IDENTITY CASCADE")
        .execute(&pool)
        .await
        .expect("truncate device tables");
    pool
}

struct Harness {
    base: String,
    repos_root: PathBuf,
    client: reqwest::Client,
    _tmp: tempfile::TempDir,
}

impl Harness {
    async fn new() -> Self {
        let pool = fresh_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let repos_root = tmp.path().to_path_buf();
        let (state, _runtime) = common::state_with_loopback_node(
            pool,
            &repos_root,
            &tmp.path().join(".workspaces"),
            &tmp.path().join(".library"),
        )
        .await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = app(state);
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            base: format!("http://{addr}"),
            repos_root,
            client: reqwest::Client::new(),
            _tmp: tmp,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Create an (empty) repo directory so `repo_path` resolves.
    /// Creates the repo the way a client does. A bare `mkdir` is not enough:
    /// repo routes resolve the owning node from the `repos` row, which only
    /// exists once the node has created or discovered the repo.
    async fn make_repo(&self, name: &str) {
        let response = self
            .client
            .post(self.url("/api/repos"))
            .json(&json!({ "name": name }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    }

    /// Drive pairing all the way to a usable device token.
    async fn obtain_token(&self) -> String {
        let start: serde_json::Value = self
            .client
            .post(self.url("/api/devices/pair"))
            .json(&json!({ "client": "ableton-extensions" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let device_code = start["device_code"].as_str().unwrap().to_string();
        let user_code = start["user_code"].as_str().unwrap().to_string();

        let approve = self
            .client
            .post(self.url("/api/devices/pair/approve"))
            .json(&json!({ "user_code": user_code }))
            .send()
            .await
            .unwrap();
        assert_eq!(approve.status(), 200);

        let tok: serde_json::Value = self
            .client
            .post(self.url("/api/devices/pair/token"))
            .json(&json!({ "device_code": device_code }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        tok["access_token"].as_str().unwrap().to_string()
    }
}

#[tokio::test]
async fn pairing_then_ingest_writes_file_to_repo() {
    let h = Harness::new().await;
    h.make_repo("atlas").await;

    // Poll before approval → 428 authorization_pending (covers the pending path).
    let start: serde_json::Value = h
        .client
        .post(h.url("/api/devices/pair"))
        .json(&json!({ "client": "probe" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pending = h
        .client
        .post(h.url("/api/devices/pair/token"))
        .json(&json!({ "device_code": start["device_code"].as_str().unwrap() }))
        .send()
        .await
        .unwrap();
    assert_eq!(pending.status(), reqwest::StatusCode::PRECONDITION_REQUIRED);

    // Full pairing → token, then write a file under the repo.
    let token = h.obtain_token().await;
    let content = b"MThd\x00\x00\x00\x06 not really midi but binary-ish \x00\x01";

    let resp = h
        .client
        .post(h.url("/api/repos/atlas/ingest?path=clips/verse.mid"))
        .bearer_auth(&token)
        .body(content.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["path"], "clips/verse.mid");
    assert_eq!(body["bytes"], content.len());

    // The bytes really landed on disk under the repo, intact.
    let written = std::fs::read(h.repos_root.join("atlas").join("clips/verse.mid")).unwrap();
    assert_eq!(written, content);

    // Read it back via the device-authed raw download — bytes round-trip.
    let raw = h
        .client
        .get(h.url("/api/repos/atlas/raw?path=clips/verse.mid"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(raw.status(), 200);
    assert_eq!(
        raw.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/octet-stream"),
    );
    assert_eq!(raw.bytes().await.unwrap().as_ref(), content);

    // Missing file → 404.
    let missing = h
        .client
        .get(h.url("/api/repos/atlas/raw?path=clips/nope.mid"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    // Path traversal is rejected on both write and read.
    let escape = h
        .client
        .post(h.url("/api/repos/atlas/ingest?path=../escape.txt"))
        .bearer_auth(&token)
        .body(b"x".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(escape.status(), 400);

    let escape_read = h
        .client
        .get(h.url("/api/repos/atlas/raw?path=../../etc/passwd"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(escape_read.status(), 400);
}

#[tokio::test]
async fn ingest_rejects_missing_and_bad_tokens() {
    let h = Harness::new().await;
    h.make_repo("atlas").await;

    // No Authorization header.
    let resp = h
        .client
        .post(h.url("/api/repos/atlas/ingest?path=x.txt"))
        .body(b"data".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Bogus bearer token.
    let resp = h
        .client
        .post(h.url("/api/repos/atlas/ingest?path=x.txt"))
        .bearer_auth("not-a-real-token")
        .body(b"data".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // The raw download is behind the same device-token layer.
    let no_token = h
        .client
        .get(h.url("/api/repos/atlas/raw?path=x.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(no_token.status(), 401);
}

#[tokio::test]
async fn approve_unknown_code_is_not_found() {
    let h = Harness::new().await;
    let resp = h
        .client
        .post(h.url("/api/devices/pair/approve"))
        .json(&json!({ "user_code": "ZZZZ-9999" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}
