//! REST API integration tests: full axum stack, real Postgres, real
//! filesystem for repo scans. Gated on `SHUTTLECRAFT_TEST_DB`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use shuttlecraft::db;
use shuttlecraft::{app, AppState};
use tokio::net::TcpListener;
use uuid::Uuid;

fn test_db_url() -> Option<String> {
    std::env::var("SHUTTLECRAFT_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SHUTTLECRAFT_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    sqlx::query(
        "TRUNCATE events, ingester_state, claude_sessions, pty_sessions, repos RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .ok();
    db::run_migrations(&pool).await.expect("migrate");
    pool
}

struct Harness {
    base: String,
    state: Arc<AppState>,
    client: reqwest::Client,
    _tmp_repos: tempfile::TempDir,
}

impl Harness {
    async fn new() -> Self {
        let pool = fresh_pool().await;
        let tmp_repos = tempfile::tempdir().unwrap();
        let state = AppState::new(
            pool,
            tmp_repos.path().to_path_buf(),
            std::sync::Arc::new(shuttlecraft::ingester::Ingester::new()),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = app(state.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            base: format!("http://{addr}"),
            state,
            client: reqwest::Client::new(),
            _tmp_repos: tmp_repos,
        }
    }

    fn repos_root(&self) -> PathBuf {
        self.state.repos_root.clone()
    }

    async fn shutdown_sessions(&self) {
        // Best-effort cleanup so later tests don't trip over live PTYs.
        if let Ok(list) = self.state.pty.list().await {
            for meta in list {
                let _ = self.state.pty.delete(meta.id).await;
            }
        }
    }
}

#[tokio::test]
#[ignore]
async fn sessions_crud_roundtrip() {
    let h = Harness::new().await;
    // Create a repo dir so working_dir is valid.
    let repo_name = "testrepo";
    std::fs::create_dir_all(h.repos_root().join(repo_name)).unwrap();

    // POST /api/sessions
    let resp = h
        .client
        .post(format!("{}/api/sessions", h.base))
        .json(&json!({ "repo": repo_name }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let created: serde_json::Value = resp.json().await.unwrap();
    let id = created["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    assert_eq!(created["state"], "live");
    assert_eq!(created["repo"], repo_name);

    // GET /api/sessions
    let list: serde_json::Value = h
        .client
        .get(format!("{}/api/sessions", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sessions = list["sessions"].as_array().unwrap();
    assert!(sessions.iter().any(|s| s["id"] == created["id"]));

    // DELETE /api/sessions/:id
    let resp = h
        .client
        .delete(format!("{}/api/sessions/{}", h.base, id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    // Listing should omit deleted sessions.
    let list: serde_json::Value = h
        .client
        .get(format!("{}/api/sessions", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sessions = list["sessions"].as_array().unwrap();
    assert!(
        !sessions.iter().any(|s| s["id"] == created["id"]),
        "deleted session must not reappear in list"
    );
}

#[tokio::test]
#[ignore]
async fn create_session_with_missing_repo_returns_400() {
    let h = Harness::new().await;
    let resp = h
        .client
        .post(format!("{}/api/sessions", h.base))
        .json(&json!({ "repo": "no-such-repo" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
#[ignore]
async fn history_returns_events_after_ingest_and_correlate() {
    let h = Harness::new().await;

    // Create PTY via the API so we have a real pty row.
    std::fs::create_dir_all(h.repos_root().join("r")).unwrap();
    let created: serde_json::Value = h
        .client
        .post(format!("{}/api/sessions", h.base))
        .json(&json!({ "repo": "r" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pty_id = created["id"].as_str().unwrap().parse::<Uuid>().unwrap();

    // Fake a correlation (like the SessionStart hook would).
    let claude_uuid = Uuid::new_v4();
    shuttlecraft::correlate::apply(
        &h.state.pool,
        &shuttlecraft::correlate::CorrelateMsg {
            pty_id,
            session_uuid: claude_uuid,
            agent: "claude-code".to_string(),
        },
    )
    .await
    .unwrap();

    // Insert events directly (bypassing the JSONL ingester — the REST
    // handler doesn't care where the rows came from).
    let events = [
        (
            0_i64,
            "user",
            json!({"msg": "hello"}),
            Some("user"),
            Some("text"),
            Some("evt-user-1"),
            None::<&str>,
            None::<&str>,
            false,
            false,
            None::<&str>,
        ),
        (
            120_i64,
            "assistant",
            json!({"msg": "hi!"}),
            Some("assistant"),
            Some("text"),
            Some("evt-assistant-1"),
            None::<&str>,
            None::<&str>,
            false,
            false,
            None::<&str>,
        ),
        (
            240_i64,
            "tool_use",
            json!({"name": "Read"}),
            Some("assistant"),
            Some("tool_use"),
            Some("evt-tool-1"),
            Some("evt-assistant-1"),
            None::<&str>,
            false,
            false,
            None::<&str>,
        ),
    ];
    for (
        offset,
        kind,
        payload,
        speaker,
        content_kind,
        event_uuid,
        parent_event_uuid,
        related_tool_use_id,
        is_sidechain,
        is_meta,
        subtype,
    ) in &events
    {
        sqlx::query(
            "INSERT INTO events \
             (session_uuid, byte_offset, timestamp, kind, payload, agent, speaker, content_kind, \
              event_uuid, parent_event_uuid, related_tool_use_id, is_sidechain, is_meta, subtype, search_text) \
             VALUES ($1, $2, NOW(), $3, $4, 'claude-code', $5, $6, $7, $8, $9, $10, $11, $12, '')",
        )
        .bind(claude_uuid)
        .bind(offset)
        .bind(kind)
        .bind(payload)
        .bind(speaker)
        .bind(content_kind)
        .bind(event_uuid)
        .bind(parent_event_uuid)
        .bind(related_tool_use_id)
        .bind(is_sidechain)
        .bind(is_meta)
        .bind(subtype)
        .execute(&h.state.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO event_blocks \
         (session_uuid, byte_offset, ord, kind, text, tool_id, tool_name, tool_name_canonical, tool_input, is_error, raw) \
         VALUES \
         ($1, 0, 0, 'text', 'hello', NULL, NULL, NULL, NULL, NULL, NULL), \
         ($1, 120, 0, 'text', 'hi!', NULL, NULL, NULL, NULL, NULL, NULL), \
         ($1, 240, 0, 'tool_use', NULL, 'toolu_1', 'Read', 'read', '{\"path\":\"/etc/hosts\"}'::jsonb, NULL, '{\"debug\":true}'::jsonb)",
    )
    .bind(claude_uuid)
    .execute(&h.state.pool)
    .await
    .unwrap();

    // GET history — no filter
    let body: serde_json::Value = h
        .client
        .get(format!("{}/api/sessions/{}/history", h.base, pty_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["session_uuid"], claude_uuid.to_string());
    assert_eq!(body["session_agent"], "claude-code");
    assert_eq!(body["events"].as_array().unwrap().len(), 3);
    let first = body["events"][0].as_object().unwrap();
    assert!(
        !first.contains_key("payload"),
        "history response must not expose raw events.payload"
    );
    assert_eq!(body["events"][0]["speaker"], "user");
    assert_eq!(body["events"][0]["content_kind"], "text");
    assert_eq!(body["events"][0]["event_uuid"], "evt-user-1");
    assert_eq!(body["events"][0]["blocks"][0]["kind"], "text");
    assert_eq!(body["events"][0]["blocks"][0]["text"], "hello");
    assert_eq!(body["events"][2]["parent_event_uuid"], "evt-assistant-1");
    assert_eq!(
        body["events"][2]["blocks"][0]["tool_name_canonical"],
        "read"
    );
    assert_eq!(
        body["events"][2]["blocks"][0]["tool_input"]["path"],
        "/etc/hosts"
    );
    assert!(
        body["events"][2]["blocks"][0].get("raw").is_none(),
        "history response must not expose raw event_blocks.raw"
    );

    // Filter by kind
    let body: serde_json::Value = h
        .client
        .get(format!(
            "{}/api/sessions/{}/history?kind=assistant",
            h.base, pty_id
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["events"].as_array().unwrap().len(), 1);
    assert_eq!(body["events"][0]["kind"], "assistant");

    // Pagination: after=120 should return only the event at 240
    let body: serde_json::Value = h
        .client
        .get(format!(
            "{}/api/sessions/{}/history?after=120",
            h.base, pty_id
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ev = body["events"].as_array().unwrap();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["byte_offset"], 240);

    h.shutdown_sessions().await;
}

#[tokio::test]
#[ignore]
async fn history_with_no_current_session_returns_empty() {
    let h = Harness::new().await;
    std::fs::create_dir_all(h.repos_root().join("r")).unwrap();
    let created: serde_json::Value = h
        .client
        .post(format!("{}/api/sessions", h.base))
        .json(&json!({ "repo": "r" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pty_id = created["id"].as_str().unwrap();

    let body: serde_json::Value = h
        .client
        .get(format!("{}/api/sessions/{}/history", h.base, pty_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["events"].as_array().unwrap().len(), 0);
    assert!(body["session_uuid"].is_null());
    assert!(body["session_agent"].is_null());

    h.shutdown_sessions().await;
}

#[tokio::test]
#[ignore]
async fn history_on_unknown_session_returns_404() {
    let h = Harness::new().await;
    let resp = h
        .client
        .get(format!(
            "{}/api/sessions/{}/history",
            h.base,
            Uuid::new_v4()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
#[ignore]
async fn repos_list_reflects_directory_scan() {
    let h = Harness::new().await;
    std::fs::create_dir_all(h.repos_root().join("aaa")).unwrap();
    std::fs::create_dir_all(h.repos_root().join("bbb")).unwrap();
    std::fs::create_dir_all(h.repos_root().join(".hidden")).unwrap();

    let body: serde_json::Value = h
        .client
        .get(format!("{}/api/repos", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = body["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["aaa", "bbb"]);
}

#[tokio::test]
#[ignore]
async fn create_repo_init_creates_git_dir() {
    let h = Harness::new().await;
    let resp = h
        .client
        .post(format!("{}/api/repos", h.base))
        .json(&json!({ "name": "freshy" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "freshy");
    assert!(h.repos_root().join("freshy/.git").exists());
}

#[tokio::test]
#[ignore]
async fn create_repo_rejects_duplicate_and_invalid_names() {
    let h = Harness::new().await;
    // pre-existing
    std::fs::create_dir_all(h.repos_root().join("x")).unwrap();
    let resp = h
        .client
        .post(format!("{}/api/repos", h.base))
        .json(&json!({ "name": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // invalid name — contains slash
    let resp = h
        .client
        .post(format!("{}/api/repos", h.base))
        .json(&json!({ "name": "bad/name" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
#[ignore]
async fn health_endpoint_reports_ok_when_db_reachable() {
    let h = Harness::new().await;
    let resp = h
        .client
        .get(format!("{}/health", h.base))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["db"], "ok");
}
