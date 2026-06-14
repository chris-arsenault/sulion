#![cfg(feature = "integration-tests")]

//! Device-pairing + MIDI-ingest integration tests: full axum stack, real
//! Postgres. Auth is disabled (the harness uses no `AuthState`), so the
//! approval handler binds to the synthetic `dev` principal. Gated on
//! `SULION_TEST_DB`.

use std::sync::Arc;

use serde_json::json;
use sulion::{app, db, AppState};
use tokio::net::TcpListener;
use uuid::Uuid;

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    db::run_migrations(&pool).await.expect("migrate");
    sqlx::query("TRUNCATE device_pairings, device_tokens, midi_clips RESTART IDENTITY CASCADE")
        .execute(&pool)
        .await
        .expect("truncate device tables");
    pool
}

struct Harness {
    base: String,
    pool: db::Pool,
    client: reqwest::Client,
    _tmp: tempfile::TempDir,
}

impl Harness {
    async fn new() -> Self {
        let pool = fresh_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(
            pool.clone(),
            tmp.path().to_path_buf(),
            tmp.path().join(".workspaces"),
            tmp.path().join(".library"),
            Arc::new(sulion::ingest::Ingester::new()),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = app(state);
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            base: format!("http://{addr}"),
            pool,
            client: reqwest::Client::new(),
            _tmp: tmp,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }
}

#[tokio::test]
async fn pairing_then_ingest_roundtrip() {
    let h = Harness::new().await;

    // 1. Start pairing.
    let resp = h
        .client
        .post(h.url("/api/devices/pair"))
        .json(&json!({ "client": "ableton-extensions" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let start: serde_json::Value = resp.json().await.unwrap();
    let device_code = start["device_code"].as_str().unwrap().to_string();
    let user_code = start["user_code"].as_str().unwrap().to_string();
    assert_eq!(start["interval"], 2);
    assert!(start["verification_uri_complete"]
        .as_str()
        .unwrap()
        .contains(&user_code));

    // 2. Poll before approval → 428 authorization_pending.
    let resp = h
        .client
        .post(h.url("/api/devices/pair/token"))
        .json(&json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::PRECONDITION_REQUIRED);

    // 3. Approve (auth disabled → dev principal).
    let resp = h
        .client
        .post(h.url("/api/devices/pair/approve"))
        .json(&json!({ "user_code": user_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let approved: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(approved["status"], "approved");
    assert_eq!(approved["client"], "ableton-extensions");

    // 4. Poll again → token minted.
    let resp = h
        .client
        .post(h.url("/api/devices/pair/token"))
        .json(&json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let tok: serde_json::Value = resp.json().await.unwrap();
    let access_token = tok["access_token"].as_str().unwrap().to_string();
    assert_eq!(tok["token_type"], "Bearer");

    // 5. Re-polling the claimed device_code → 410 Gone.
    let resp = h
        .client
        .post(h.url("/api/devices/pair/token"))
        .json(&json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::GONE);

    // 6. Ingest a clip with the device token.
    let resp = h
        .client
        .post(h.url("/api/midi/ingest"))
        .bearer_auth(&access_token)
        .json(&json!({
            "source": "ableton",
            "name": "Verse bassline",
            "tempo": 120.0,
            "lengthBeats": 4.0,
            "timeSignature": { "numerator": 4, "denominator": 4 },
            "notes": [
                { "pitch": 36, "start": 0.0, "duration": 0.5, "velocity": 100, "muted": false },
                { "pitch": 36, "start": 1.0, "duration": 0.5, "velocity": 90 }
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ingest: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(ingest["note_count"], 2);
    let ingest_id = ingest["ingest_id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();

    // 7. Verify it persisted, bound to the dev user, with notes intact.
    let row = sqlx::query_as::<_, (String, i32, String)>(
        "SELECT user_sub, note_count, source FROM midi_clips WHERE ingest_id = $1",
    )
    .bind(ingest_id)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(row.0, "dev");
    assert_eq!(row.1, 2);
    assert_eq!(row.2, "ableton");
}

#[tokio::test]
async fn ingest_rejects_missing_and_bad_tokens() {
    let h = Harness::new().await;
    let body =
        json!({ "notes": [ { "pitch": 60, "start": 0.0, "duration": 1.0, "velocity": 100 } ] });

    // No Authorization header.
    let resp = h
        .client
        .post(h.url("/api/midi/ingest"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Bogus bearer token.
    let resp = h
        .client
        .post(h.url("/api/midi/ingest"))
        .bearer_auth("not-a-real-token")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
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
