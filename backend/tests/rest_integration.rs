#![cfg(feature = "integration-tests")]

//! REST API integration tests: full axum stack, real Postgres, real
//! filesystem for repo scans. Gated on `SULION_TEST_DB`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde_json::json;
use sulion::db;
use sulion::{app, AppState};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use uuid::Uuid;

type RetrievalAdminMockSeen = Arc<Mutex<Vec<(Option<String>, serde_json::Value)>>>;

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    db::run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE retrieval_embedding_backfills, retrieval_embedding_sources, retrieval_embeddings, \
         plan_events, plan_attachments, plan_phases, plans, session_activity_state, \
         events, ingester_state, claude_sessions, pty_sessions, repos, \
         repo_runtime_state, repo_dirty_paths, timeline_session_state, \
         future_prompt_session_state, workspaces, workspace_dirty_paths RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate test tables");
    pool
}

async fn insert_test_pty(pool: &db::Pool, repo: &str, working_dir: &Path) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state, created_at) \
         VALUES ($1, $2, $3, 'live', NOW())",
    )
    .bind(id)
    .bind(repo)
    .bind(working_dir.to_string_lossy().as_ref())
    .execute(pool)
    .await
    .unwrap();
    id
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
            tmp_repos.path().join(".workspaces"),
            tmp_repos.path().join(".library"),
            std::sync::Arc::new(sulion::ingest::Ingester::new()),
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
async fn legacy_ambient_poll_contracts_are_removed() {
    let h = Harness::new().await;
    for (path, expected) in [
        ("/api/sessions", reqwest::StatusCode::METHOD_NOT_ALLOWED),
        ("/api/repos", reqwest::StatusCode::METHOD_NOT_ALLOWED),
        ("/api/stats", reqwest::StatusCode::NOT_FOUND),
        ("/api/repos/r/git", reqwest::StatusCode::NOT_FOUND),
    ] {
        let resp = h
            .client
            .get(format!("{}{}", h.base, path))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), expected, "{path}");
    }
}

#[tokio::test]
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

    // GET /api/app-state is the ambient session/repo/status contract.
    let list: serde_json::Value = h
        .client
        .get(format!("{}/api/app-state", h.base))
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
        .get(format!("{}/api/app-state", h.base))
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
async fn app_state_includes_agent_usage_health_metrics() {
    let h = Harness::new().await;
    let repo_path = h.repos_root().join("usage-repo");
    std::fs::create_dir_all(&repo_path).unwrap();
    let pty_id = insert_test_pty(&h.state.pool, "usage-repo", &repo_path).await;
    let session_uuid = Uuid::new_v4();
    sulion::correlate::apply(
        &h.state.pool,
        &sulion::correlate::CorrelateMsg {
            pty_id,
            session_uuid,
            agent: "codex".to_string(),
        },
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent_session_metadata \
            (session_uuid, agent, model, model_context_window) \
         VALUES ($1, 'codex', 'gpt-5.4', 100000)",
    )
    .bind(session_uuid)
    .execute(&h.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent_session_usage \
            (session_uuid, agent, input_tokens, cached_input_tokens, output_tokens, \
             reasoning_output_tokens, total_tokens, context_tokens, model_context_window, \
             last_byte_offset, observed_at) \
         VALUES ($1, 'codex', 42000, 31000, 5000, 1800, 47000, 26000, NULL, 120, NOW())",
    )
    .bind(session_uuid)
    .execute(&h.state.pool)
    .await
    .unwrap();

    let state: serde_json::Value = h
        .client
        .get(format!("{}/api/app-state", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = state["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["id"] == pty_id.to_string())
        .unwrap();

    assert_eq!(session["agent_usage"]["total_tokens"], 47_000);
    assert_eq!(session["agent_usage"]["cached_input_tokens"], 31_000);
    assert_eq!(session["agent_usage"]["context_tokens"], 26_000);
    assert_eq!(session["agent_usage"]["model_context_window"], 100_000);
}

#[tokio::test]
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
async fn admin_retrieval_reindex_proxies_to_retrieval_service() {
    let h = Harness::new().await;
    let seen = Arc::new(Mutex::new(Vec::<(Option<String>, serde_json::Value)>::new()));
    let mock_url = start_retrieval_admin_mock(seen.clone()).await;
    let old_url = std::env::var_os("SULION_RETRIEVAL_URL");
    let old_token = std::env::var_os("SULION_RETRIEVAL_TOKEN");
    std::env::set_var("SULION_RETRIEVAL_URL", mock_url);
    std::env::set_var("SULION_RETRIEVAL_TOKEN", "admin-token");

    let resp = h
        .client
        .post(format!("{}/api/admin/retrieval/reindex", h.base))
        .json(&json!({ "repo": " sulion " }))
        .send()
        .await
        .unwrap();

    restore_env("SULION_RETRIEVAL_URL", old_url);
    restore_env("SULION_RETRIEVAL_TOKEN", old_token);
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["generation"], 4);
    assert_eq!(body["backfills_started"], 3);
    assert_eq!(body["sources_seen"], 12);
    assert_eq!(body["sources_marked_pending"], 12);
    assert_eq!(body["sources_deleted"], 1);
    assert_eq!(body["pending_sources"], 12);
    assert_eq!(body["embedding_model"], "test-embed");

    let seen = seen.lock().await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0.as_deref(), Some("Bearer admin-token"));
    assert_eq!(seen[0].1["repo"], "sulion");
    assert!(seen[0].1.get("limit").is_none());
}

async fn start_retrieval_admin_mock(seen: RetrievalAdminMockSeen) -> String {
    async fn handler(
        axum::extract::State(seen): axum::extract::State<RetrievalAdminMockSeen>,
        headers: HeaderMap,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> axum::Json<serde_json::Value> {
        let auth = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let mut seen = seen.lock().await;
        seen.push((auth, body));
        axum::Json(json!({
            "generation": 4,
            "backfills_started": 3,
            "sources_seen": 12,
            "sources_marked_pending": 12,
            "sources_deleted": 1,
            "pending_sources": 12,
            "vector": {
                "extension_installed": true,
                "column_exists": true,
                "ann_index_exists": true
            },
            "embedding_model": "test-embed",
            "embedding_dimensions": 768
        }))
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = axum::Router::new()
        .route("/v1/reindex", axum::routing::post(handler))
        .with_state(seen);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}")
}

fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
    if let Some(value) = value {
        std::env::set_var(key, value);
    } else {
        std::env::remove_var(key);
    }
}

#[tokio::test]
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
    sulion::correlate::apply(
        &h.state.pool,
        &sulion::correlate::CorrelateMsg {
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
            json!({"name": "functions.exec_command"}),
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
         ($1, 240, 0, 'tool_use', NULL, 'toolu_1', 'functions.exec_command', 'functions.exec_command', '{\"cmd\":\"ls -la\"}'::jsonb, NULL, '{\"debug\":true}'::jsonb)",
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
        "functions.exec_command"
    );
    assert_eq!(
        body["events"][2]["blocks"][0]["operation_type"],
        "exec_command"
    );
    assert_eq!(
        body["events"][2]["blocks"][0]["operation_category"],
        "utility"
    );
    assert_eq!(
        body["events"][2]["blocks"][0]["tool_input"]["cmd"],
        "ls -la"
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
async fn timeline_returns_projected_turns() {
    let h = Harness::new().await;

    std::fs::create_dir_all(h.repos_root().join("r")).unwrap();
    let pty_id = insert_test_pty(&h.state.pool, "r", &h.repos_root().join("r")).await;

    let session_uuid = Uuid::new_v4();
    sulion::correlate::apply(
        &h.state.pool,
        &sulion::correlate::CorrelateMsg {
            pty_id,
            session_uuid,
            agent: "claude-code".to_string(),
        },
    )
    .await
    .unwrap();

    for (offset, kind, speaker, content_kind, event_uuid, parent_event_uuid) in [
        (
            0_i64,
            "user",
            Some("user"),
            Some("text"),
            Some("evt-user"),
            None::<&str>,
        ),
        (
            120_i64,
            "assistant",
            Some("assistant"),
            Some("mixed"),
            Some("evt-assistant"),
            None::<&str>,
        ),
        (
            240_i64,
            "user",
            Some("user"),
            Some("tool_result"),
            Some("evt-result"),
            Some("evt-assistant"),
        ),
    ] {
        sqlx::query(
            "INSERT INTO events \
             (session_uuid, byte_offset, timestamp, kind, payload, agent, speaker, content_kind, \
              event_uuid, parent_event_uuid, related_tool_use_id, is_sidechain, is_meta, subtype, search_text) \
             VALUES ($1, $2, NOW(), $3, '{}'::jsonb, 'claude-code', $4, $5, $6, $7, NULL, false, false, NULL, '')",
        )
        .bind(session_uuid)
        .bind(offset)
        .bind(kind)
        .bind(speaker)
        .bind(content_kind)
        .bind(event_uuid)
        .bind(parent_event_uuid)
        .execute(&h.state.pool)
        .await
        .unwrap();
    }

    sqlx::query(
        "INSERT INTO event_blocks \
         (session_uuid, byte_offset, ord, kind, text, tool_id, tool_name, tool_name_canonical, tool_input, tool_output, is_error, raw) \
         VALUES \
         ($1, 0, 0, 'text', 'hello', NULL, NULL, NULL, NULL, NULL, NULL, NULL), \
         ($1, 120, 0, 'text', 'running command', NULL, NULL, NULL, NULL, NULL, NULL, NULL), \
         ($1, 120, 1, 'tool_use', NULL, 'toolu_1', 'Read', 'read', '{\"path\":\"src/lib.rs\"}'::jsonb, NULL, NULL, NULL), \
         ($1, 240, 0, 'tool_result', 'fn main() {}', 'toolu_1', NULL, NULL, NULL, '{\"path\":\"src/lib.rs\",\"old_text\":\"fn old() {}\",\"new_text\":\"fn main() {}\"}'::jsonb, false, NULL)",
    )
    .bind(session_uuid)
    .execute(&h.state.pool)
    .await
    .unwrap();

    sulion::ingest::rebuild_session_projection(&h.state.pool, session_uuid)
        .await
        .unwrap();
    h.state
        .repo_state
        .upsert_repo("r", &h.repos_root().join("r"))
        .await
        .unwrap();

    sqlx::query(
        "UPDATE timeline_turns \
            SET turn_json = $2 \
          WHERE session_uuid = $1",
    )
    .bind(session_uuid)
    .bind(json!({
        "id": 999,
        "preview": "stale turn json",
        "user_prompt_text": "wrong prompt",
        "start_timestamp": "2025-01-01T00:00:00Z",
        "end_timestamp": "2025-01-01T00:00:00Z",
        "duration_ms": 0,
        "event_count": 0,
        "operation_count": 0,
        "tool_pairs": [],
        "thinking_count": 0,
        "has_errors": false,
        "markdown": "wrong",
        "chunks": [],
    }))
    .execute(&h.state.pool)
    .await
    .unwrap();

    let response = h
        .client
        .get(format!("{}/api/sessions/{}/timeline", h.base, pty_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();

    assert_eq!(body["session_uuid"], session_uuid.to_string());
    assert_eq!(body["session_agent"], "claude-code");
    assert_eq!(body["total_event_count"], 3);
    assert_eq!(body["turns"].as_array().unwrap().len(), 1);
    assert_eq!(body["turns"][0]["preview"], "hello");
    assert_eq!(body["turns"][0]["session_uuid"], session_uuid.to_string());
    assert_eq!(body["turns"][0]["pty_session_id"], pty_id.to_string());
    assert_eq!(body["turns"][0]["operation_count"], 1);
    assert_eq!(body["turns"][0]["operation_badges"][0]["name"], "read");
    assert_eq!(body["turns"][0]["operation_badges"][0]["count"], 1);

    let detail_response = h
        .client
        .get(format!(
            "{}/api/sessions/{}/timeline/turns/0",
            h.base, pty_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(detail_response.status(), reqwest::StatusCode::OK);
    let detail: serde_json::Value = detail_response.json().await.unwrap();
    let turn = &detail["turn"];
    assert_eq!(turn["tool_pairs"][0]["name"], "read");
    assert_eq!(turn["tool_pairs"][0]["category"], "inspect");
    assert_eq!(
        turn["tool_pairs"][0]["result"]["payload"]["old_text"],
        "fn old() {}"
    );
    assert_eq!(
        turn["tool_pairs"][0]["result"]["payload"]["new_text"],
        "fn main() {}"
    );
    assert_eq!(
        turn["tool_pairs"][0]["file_touches"][0]["path"],
        "src/lib.rs"
    );
    assert_eq!(turn["chunks"][0]["kind"], "assistant");
    assert_eq!(turn["chunks"][1]["kind"], "tool");
}

#[tokio::test]
async fn repo_timeline_returns_merged_turns_across_sessions() {
    let h = Harness::new().await;

    std::fs::create_dir_all(h.repos_root().join("r")).unwrap();

    let first_created: serde_json::Value = h
        .client
        .post(format!("{}/api/sessions", h.base))
        .json(&json!({ "repo": "r" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let first_pty_id = first_created["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();

    let second_created: serde_json::Value = h
        .client
        .post(format!("{}/api/sessions", h.base))
        .json(&json!({ "repo": "r" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second_pty_id = second_created["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();

    let first_session_uuid = Uuid::new_v4();
    sulion::correlate::apply(
        &h.state.pool,
        &sulion::correlate::CorrelateMsg {
            pty_id: first_pty_id,
            session_uuid: first_session_uuid,
            agent: "codex".to_string(),
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE pty_sessions SET label = 'one' WHERE id = $1")
        .bind(first_pty_id)
        .execute(&h.state.pool)
        .await
        .unwrap();

    let second_session_uuid = Uuid::new_v4();
    sulion::correlate::apply(
        &h.state.pool,
        &sulion::correlate::CorrelateMsg {
            pty_id: second_pty_id,
            session_uuid: second_session_uuid,
            agent: "claude-code".to_string(),
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE pty_sessions SET label = 'two' WHERE id = $1")
        .bind(second_pty_id)
        .execute(&h.state.pool)
        .await
        .unwrap();

    let ts_one = DateTime::parse_from_rfc3339("2026-04-20T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let ts_two = DateTime::parse_from_rfc3339("2026-04-20T01:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    for (session_uuid, ts, prompt) in [
        (first_session_uuid, ts_one, "first prompt"),
        (second_session_uuid, ts_two, "second prompt"),
    ] {
        sqlx::query(
            "INSERT INTO events \
             (session_uuid, byte_offset, timestamp, kind, payload, agent, speaker, content_kind, \
              event_uuid, parent_event_uuid, related_tool_use_id, is_sidechain, is_meta, subtype, search_text) \
             VALUES ($1, 0, $2, 'user', '{}'::jsonb, 'codex', 'user', 'text', NULL, NULL, NULL, false, false, NULL, '')",
        )
        .bind(session_uuid)
        .bind(ts)
        .execute(&h.state.pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO event_blocks \
             (session_uuid, byte_offset, ord, kind, text, tool_id, tool_name, tool_name_canonical, tool_input, is_error, raw) \
             VALUES ($1, 0, 0, 'text', $2, NULL, NULL, NULL, NULL, NULL, NULL)",
        )
        .bind(session_uuid)
        .bind(prompt)
        .execute(&h.state.pool)
        .await
        .unwrap();

        sulion::ingest::rebuild_session_projection(&h.state.pool, session_uuid)
            .await
            .unwrap();
    }

    let body: serde_json::Value = h
        .client
        .get(format!("{}/api/repos/r/timeline", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(body["session_uuid"].is_null());
    assert!(body["session_agent"].is_null());
    assert_eq!(body["turns"].as_array().unwrap().len(), 2);
    assert_eq!(body["turns"][0]["preview"], "first prompt");
    assert_eq!(body["turns"][0]["session_label"], "one");
    assert_eq!(body["turns"][0]["pty_session_id"], first_pty_id.to_string());
    assert_eq!(body["turns"][1]["preview"], "second prompt");
    assert_eq!(body["turns"][1]["session_label"], "two");
    assert_eq!(
        body["turns"][1]["pty_session_id"],
        second_pty_id.to_string()
    );

    h.shutdown_sessions().await;
}

#[tokio::test]
async fn file_trace_returns_related_turns() {
    let h = Harness::new().await;

    std::fs::create_dir_all(h.repos_root().join("r/src")).unwrap();
    std::fs::write(h.repos_root().join("r/src/lib.rs"), "fn main() {}\n").unwrap();
    let pty_id = insert_test_pty(&h.state.pool, "r", &h.repos_root().join("r")).await;

    let session_uuid = Uuid::new_v4();
    sulion::correlate::apply(
        &h.state.pool,
        &sulion::correlate::CorrelateMsg {
            pty_id,
            session_uuid,
            agent: "codex".to_string(),
        },
    )
    .await
    .unwrap();

    for (offset, kind, speaker, content_kind) in [
        (0_i64, "user", Some("user"), Some("text")),
        (120_i64, "assistant", Some("assistant"), Some("mixed")),
        (240_i64, "user", Some("user"), Some("tool_result")),
    ] {
        sqlx::query(
            "INSERT INTO events \
             (session_uuid, byte_offset, timestamp, kind, payload, agent, speaker, content_kind, \
              event_uuid, parent_event_uuid, related_tool_use_id, is_sidechain, is_meta, subtype, search_text) \
             VALUES ($1, $2, NOW(), $3, '{}'::jsonb, 'codex', $4, $5, NULL, NULL, NULL, false, false, NULL, '')",
        )
        .bind(session_uuid)
        .bind(offset)
        .bind(kind)
        .bind(speaker)
        .bind(content_kind)
        .execute(&h.state.pool)
        .await
        .unwrap();
    }

    sqlx::query(
        "INSERT INTO event_blocks \
         (session_uuid, byte_offset, ord, kind, text, tool_id, tool_name, tool_name_canonical, tool_input, is_error, raw) \
         VALUES \
         ($1, 0, 0, 'text', 'inspect file', NULL, NULL, NULL, NULL, NULL, NULL), \
         ($1, 120, 0, 'tool_use', NULL, 'toolu_1', 'read_file', 'read', '{\"path\":\"src/lib.rs\"}'::jsonb, NULL, NULL), \
         ($1, 240, 0, 'tool_result', 'fn main() {}', 'toolu_1', NULL, NULL, NULL, false, NULL)",
    )
    .bind(session_uuid)
    .execute(&h.state.pool)
    .await
    .unwrap();

    sulion::ingest::rebuild_session_projection(&h.state.pool, session_uuid)
        .await
        .unwrap();
    h.state
        .repo_state
        .upsert_repo("r", &h.repos_root().join("r"))
        .await
        .unwrap();

    let response = h
        .client
        .get(format!(
            "{}/api/repos/r/file-trace?path=src%2Flib.rs",
            h.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();

    assert_eq!(body["path"], "src/lib.rs");
    assert_eq!(body["touches"].as_array().unwrap().len(), 1);
    assert_eq!(body["touches"][0]["pty_session_id"], pty_id.to_string());
    assert_eq!(body["touches"][0]["turn_id"], 0);
    assert_eq!(body["touches"][0]["touch_kind"], "inspect");
}

#[tokio::test]
async fn app_state_repos_reflect_materialized_directory_scan() {
    let h = Harness::new().await;
    std::fs::create_dir_all(h.repos_root().join("aaa")).unwrap();
    std::fs::create_dir_all(h.repos_root().join("bbb")).unwrap();
    std::fs::create_dir_all(h.repos_root().join(".hidden")).unwrap();
    h.state.repo_state.sync_repos_once().await.unwrap();

    let body: serde_json::Value = h
        .client
        .get(format!("{}/api/app-state", h.base))
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
async fn rename_repo_moves_checkout_and_updates_session_records() {
    let h = Harness::new().await;
    let old_path = h.repos_root().join("oldrepo");
    std::fs::create_dir_all(old_path.join("src")).unwrap();
    h.state
        .repo_state
        .upsert_repo("oldrepo", &old_path)
        .await
        .unwrap();
    let pty_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state, created_at) \
         VALUES ($1, 'oldrepo', $2, 'dead', NOW())",
    )
    .bind(pty_id)
    .bind(old_path.join("src").to_string_lossy().as_ref())
    .execute(&h.state.pool)
    .await
    .unwrap();

    let resp = h
        .client
        .patch(format!("{}/api/repos/oldrepo", h.base))
        .json(&json!({ "name": "newrepo" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "newrepo");
    assert!(!h.repos_root().join("oldrepo").exists());
    assert!(h.repos_root().join("newrepo/src").is_dir());

    let (repo, working_dir): (String, String) =
        sqlx::query_as("SELECT repo, working_dir FROM pty_sessions WHERE id = $1")
            .bind(pty_id)
            .fetch_one(&h.state.pool)
            .await
            .unwrap();
    assert_eq!(repo, "newrepo");
    assert_eq!(
        working_dir,
        h.repos_root()
            .join("newrepo/src")
            .to_string_lossy()
            .into_owned()
    );
}

#[tokio::test]
async fn delete_repo_requires_force_for_dirty_checkout() {
    let h = Harness::new().await;
    let resp = h
        .client
        .post(format!("{}/api/repos", h.base))
        .json(&json!({ "name": "dirtyrepo" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    std::fs::write(h.repos_root().join("dirtyrepo/notes.txt"), "dirty\n").unwrap();

    let resp = h
        .client
        .delete(format!("{}/api/repos/dirtyrepo", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    assert!(h.repos_root().join("dirtyrepo").is_dir());

    let resp = h
        .client
        .delete(format!("{}/api/repos/dirtyrepo?force=true", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    assert!(!h.repos_root().join("dirtyrepo").exists());
}

#[tokio::test]
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

#[tokio::test]
async fn published_plan_lifecycle_projects_into_app_state() {
    let h = Harness::new().await;
    let repo_name = "planned";
    let repo_path = h.repos_root().join(repo_name);
    std::fs::create_dir_all(&repo_path).unwrap();
    let pty_id = insert_test_pty(&h.state.pool, repo_name, &repo_path).await;
    sqlx::query(
        "UPDATE pty_sessions \
            SET agent_runtime_agent = 'codex', agent_runtime_state = 'running', \
                agent_runtime_started_at = NOW() \
          WHERE id = $1",
    )
    .bind(pty_id)
    .execute(&h.state.pool)
    .await
    .unwrap();
    sulion::activity::set(
        &h.state.pool,
        pty_id,
        sulion::activity::ActivityState::Working,
        Some("Implementing the API"),
        None,
        "agent",
        "explicit",
    )
    .await
    .unwrap();

    let response = h
        .client
        .post(format!("{}/api/repos/{repo_name}/plans", h.base))
        .json(&json!({
            "title": "Native plans",
            "summary": "Publish durable phases",
            "attach_pty_id": pty_id,
            "phases": [
                { "title": "Backend", "description": "Schema and service" },
                { "title": "Frontend", "description": "Plan workspace" }
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let created: serde_json::Value = response.json().await.unwrap();
    let plan_id = created["id"].as_str().unwrap();
    let first_phase_id = created["phases"][0]["id"].as_str().unwrap();
    let second_phase_id = created["phases"][1]["id"].as_str().unwrap();
    assert_eq!(created["phases"][0]["status"], "in_progress");
    assert_eq!(created["phases"][1]["status"], "pending");
    assert_eq!(
        created["attachments"][0]["pty_session_id"],
        pty_id.to_string()
    );

    let reordered: serde_json::Value = h
        .client
        .patch(format!(
            "{}/api/plans/{plan_id}/phases/{second_phase_id}",
            h.base
        ))
        .json(&json!({ "position": 1 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reordered["phases"][0]["id"], second_phase_id);
    assert_eq!(reordered["phases"][1]["id"], first_phase_id);

    let other_pty = insert_test_pty(&h.state.pool, "other-repo", &repo_path).await;
    let cross_repo = h
        .client
        .post(format!("{}/api/plans/{plan_id}/attachments", h.base))
        .json(&json!({ "pty_session_id": other_pty }))
        .send()
        .await
        .unwrap();
    assert_eq!(cross_repo.status(), 400);

    let state: serde_json::Value = h
        .client
        .get(format!("{}/api/app-state", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = state["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["id"] == pty_id.to_string())
        .unwrap();
    assert_eq!(session["activity"]["state"], "working");
    assert_eq!(session["activity"]["summary"], "Implementing the API");
    assert_eq!(session["current_plan"]["id"], plan_id);
    assert_eq!(session["current_plan"]["current_phase_title"], "Backend");
    assert_eq!(state["plans"][0]["title"], "Native plans");

    let premature = h
        .client
        .patch(format!("{}/api/plans/{plan_id}", h.base))
        .json(&json!({ "status": "completed" }))
        .send()
        .await
        .unwrap();
    assert_eq!(premature.status(), 400);

    for phase_id in [first_phase_id, second_phase_id] {
        let response = h
            .client
            .patch(format!("{}/api/plans/{plan_id}/phases/{phase_id}", h.base))
            .json(&json!({ "status": "completed", "status_note": "done" }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }

    let response = h
        .client
        .patch(format!("{}/api/plans/{plan_id}", h.base))
        .json(&json!({ "status": "completed", "note": "shipped" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let closed: serde_json::Value = response.json().await.unwrap();
    assert_eq!(closed["status"], "completed");
    assert_eq!(closed["attachments"], json!([]));

    let open: serde_json::Value = h
        .client
        .get(format!("{}/api/repos/{repo_name}/plans", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(open, json!([]));
    let all: serde_json::Value = h
        .client
        .get(format!(
            "{}/api/repos/{repo_name}/plans?include_closed=true",
            h.base
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(all[0]["status"], "completed");

    let history: serde_json::Value = h
        .client
        .get(format!("{}/api/plans/{plan_id}/events", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        history.as_array().unwrap().len() >= 6,
        "plan transitions should be auditable"
    );

    let final_state: serde_json::Value = h
        .client
        .get(format!("{}/api/app-state", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(final_state["plans"], json!([]));
    let final_session = final_state["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["id"] == pty_id.to_string())
        .unwrap();
    assert!(final_session["current_plan"].is_null());
}
