#![cfg(feature = "integration-tests")]

//! Retrieval service integration tests: real Postgres, real axum stack,
//! no external embedding service or pgvector dependency.

use chrono::Utc;
use reqwest::StatusCode;
use ring::digest;
use serde_json::json;
use sulion::{db, retrieval};
use tokio::net::TcpListener;
use uuid::Uuid;

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
         events, event_blocks, timeline_turns, \
         timeline_operations, timeline_file_touches, timeline_activity_signals, \
         timeline_session_state, claude_sessions, pty_sessions, workspaces \
         RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate retrieval test tables");
    pool
}

struct Harness {
    base: String,
    client: reqwest::Client,
    state: std::sync::Arc<retrieval::RetrievalState>,
}

impl Harness {
    async fn new(pool: db::Pool) -> Self {
        let state = retrieval::RetrievalState::from_pool_for_tests(pool, "test-token");
        Self::from_state(state).await
    }

    async fn new_with_config(pool: db::Pool, config: retrieval::RetrievalConfig) -> Self {
        let state = retrieval::RetrievalState::from_pool_with_config_for_tests(pool, config);
        Self::from_state(state).await
    }

    async fn from_state(state: std::sync::Arc<retrieval::RetrievalState>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = retrieval::app(state.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            base: format!("http://{addr}"),
            client: reqwest::Client::new(),
            state,
        }
    }

    fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.bearer_auth("test-token")
    }
}

async fn start_embedding_server() -> String {
    async fn embeddings(
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> axum::Json<serde_json::Value> {
        let input = body["input"].as_array().cloned().unwrap_or_default();
        let data = input
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let text = value.as_str().unwrap_or_default();
                let embedding = if text.contains("semantic retrieval") {
                    vec![1.0, 0.0, 0.0]
                } else if text.contains("exec_command") {
                    vec![0.0, 1.0, 0.0]
                } else {
                    vec![0.0, 0.0, 1.0]
                };
                json!({
                    "object": "embedding",
                    "index": index,
                    "embedding": embedding,
                })
            })
            .collect::<Vec<_>>();
        axum::Json(json!({
            "object": "list",
            "data": data,
            "model": "test-embed",
            "usage": { "prompt_tokens": 0, "total_tokens": 0 }
        }))
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route("/v1/embeddings", axum::routing::post(embeddings));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn retrieval_auth_is_required() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    let h = Harness::new(pool).await;
    let resp = h
        .client
        .get(format!("{}/v1/context?repo=sulion", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn context_defaults_to_repo_scope_from_header() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    let h = Harness::new(pool).await;
    let body: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/context", h.base))
                .header("x-sulion-repo", "sulion"),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["scope"], "repo");
    assert_eq!(body["repo"], "sulion");
    assert_eq!(body["repos"], json!(["sulion"]));
}

#[tokio::test]
async fn lexical_search_returns_assistant_evidence_in_current_repo() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    let session_uuid = seed_retrieval_fixture(&pool).await;
    let h = Harness::new(pool).await;

    let body: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[
                    ("q", "retrieval api"),
                    ("search_mode", "lexical"),
                    ("limit", "5"),
                ])
                .header("x-sulion-repo", "sulion"),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{body:#}");
    assert_eq!(results[0]["source_kind"], "assistant_text");
    assert_eq!(results[0]["agent_session_uuid"], session_uuid.to_string());
    assert!(results[0]["snippet"]
        .as_str()
        .unwrap()
        .contains("retrieval api"));
    assert_eq!(
        results[0]["evidence"]["file_touches"][0]["path"],
        "backend/src/retrieval.rs"
    );
    assert_eq!(
        results[0]["evidence"]["operations"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "{body:#}"
    );
}

#[tokio::test]
async fn repo_scope_uses_every_repository_in_the_pty_snapshot() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    let alpha_session = seed_retrieval_fixture_for_repo(&pool, "alpha").await;
    let beta_session = seed_retrieval_fixture_for_repo(&pool, "beta").await;
    let collection_pty_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state) \
         VALUES ($1, 'alpha', '/repo', 'dead')",
    )
    .bind(collection_pty_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pty_session_repos \
            (pty_session_id, repo_name, workspace_id, role, position) \
         VALUES ($1, 'alpha', NULL, 'primary', 0), \
                ($1, 'beta', NULL, 'additional', 1)",
    )
    .bind(collection_pty_id)
    .execute(&pool)
    .await
    .unwrap();
    let h = Harness::new(pool).await;

    let body: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[("q", "retrieval api"), ("search_mode", "lexical")])
                .header("x-sulion-pty-id", collection_pty_id.to_string()),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["context"]["repos"], json!(["alpha", "beta"]));
    let session_ids = body["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["agent_session_uuid"].as_str().unwrap().to_string())
        .collect::<std::collections::HashSet<_>>();
    assert!(session_ids.contains(&alpha_session.to_string()));
    assert!(session_ids.contains(&beta_session.to_string()));
}

#[tokio::test]
async fn default_search_excludes_tools_until_opted_in() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    seed_retrieval_fixture(&pool).await;
    let h = Harness::new(pool).await;

    let default_body: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[("q", "command output"), ("search_mode", "lexical")])
                .header("x-sulion-repo", "sulion"),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(default_body["results"].as_array().unwrap().len(), 0);

    let tool_body_without_low_value: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[
                    ("q", "command output"),
                    ("search_mode", "lexical"),
                    ("include", "tool_result"),
                ])
                .header("x-sulion-repo", "sulion"),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        tool_body_without_low_value["results"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "{tool_body_without_low_value:#}"
    );

    let tool_body: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[
                    ("q", "command output"),
                    ("search_mode", "lexical"),
                    ("include", "tool_result"),
                    ("include_low_value", "true"),
                ])
                .header("x-sulion-repo", "sulion"),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let results = tool_body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{tool_body:#}");
    assert_eq!(results[0]["source_kind"], "tool_result");
    assert_eq!(results[0]["tool"]["name"], "exec_command");
}

#[tokio::test]
async fn lexical_search_handles_oversized_text_without_vectorizing_it() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    let session_uuid = seed_retrieval_fixture(&pool).await;
    seed_oversized_retrieval_text(&pool, session_uuid).await;
    let h = Harness::new(pool).await;

    let missing: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[
                    ("q", "definitely-not-in-the-oversized-block"),
                    ("search_mode", "lexical"),
                    ("include", "assistant,tool_result"),
                    ("limit", "5"),
                ])
                .header("x-sulion-repo", "sulion"),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(missing["results"].as_array().unwrap().len(), 0);

    let matched: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[
                    ("q", "oversized lexical guard"),
                    ("search_mode", "lexical"),
                    ("include", "assistant"),
                    ("limit", "5"),
                ])
                .header("x-sulion-repo", "sulion"),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let results = matched["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{matched:#}");
    assert_eq!(results[0]["source_kind"], "assistant_text");
    assert!(
        results[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("oversized lexical guard"),
        "{matched:#}"
    );
}

#[tokio::test]
async fn tool_category_filter_requires_low_value_opt_in_for_exec() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    seed_retrieval_fixture(&pool).await;
    let h = Harness::new(pool).await;

    let default_body: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[
                    ("q", "rg semantic"),
                    ("search_mode", "lexical"),
                    ("tool_category", "utility"),
                ])
                .header("x-sulion-repo", "sulion"),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        default_body["results"].as_array().unwrap().len(),
        0,
        "{default_body:#}"
    );

    let body: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[
                    ("q", "rg semantic"),
                    ("search_mode", "lexical"),
                    ("tool_category", "utility"),
                    ("include_low_value", "true"),
                ])
                .header("x-sulion-repo", "sulion"),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{body:#}");
    assert_eq!(results[0]["source_kind"], "tool_call");
}

#[tokio::test]
async fn startup_schedules_initial_backfill_when_source_state_is_empty() {
    let Some(url) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    seed_retrieval_fixture(&pool).await;

    let config = retrieval::RetrievalConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        db_url: url,
        token: "test-token".to_string(),
        embedding_service_url: "http://127.0.0.1:1".to_string(),
        embedding_model: "test-embed".to_string(),
        embedding_dimensions: 3,
        embedding_batch_size: 8,
        embedding_max_chars: 6000,
        embedding_chunk_max: 10,
        semantic_min_score: 0.0,
        background_index_seconds: None,
    };
    let _state = retrieval::RetrievalState::from_config(config.clone())
        .await
        .unwrap();

    let scheduled: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) \
           FROM retrieval_embedding_backfills \
          WHERE status = 'running'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(scheduled, 3);

    let _second_state = retrieval::RetrievalState::from_config(config)
        .await
        .unwrap();
    let still_scheduled: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM retrieval_embedding_backfills")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(still_scheduled, 3);
}

#[tokio::test]
async fn reindex_marks_pending_and_worker_refreshes_stale_hashes() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let embedding_url = start_embedding_server().await;
    let pool = fresh_pool().await;
    let session_uuid = seed_retrieval_fixture(&pool).await;
    let h = Harness::new_with_config(
        pool.clone(),
        retrieval::RetrievalConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            db_url: String::new(),
            token: "test-token".to_string(),
            embedding_service_url: embedding_url,
            embedding_model: "test-embed".to_string(),
            embedding_dimensions: 3,
            embedding_batch_size: 8,
            embedding_max_chars: 6000,
            embedding_chunk_max: 10,
            semantic_min_score: 0.0,
            background_index_seconds: None,
        },
    )
    .await;

    let marked: serde_json::Value = h
        .auth(
            h.client
                .post(format!("{}/v1/reindex", h.base))
                .json(&json!({ "repo": "sulion", "limit": 10 })),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(marked["backfills_started"], 3, "{marked:#}");
    assert_eq!(marked["sources_seen"], 0, "{marked:#}");
    assert_eq!(marked["sources_marked_pending"], 0, "{marked:#}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM retrieval_embeddings")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );

    let refreshed: serde_json::Value = h
        .auth(
            h.client
                .post(format!("{}/v1/reindex", h.base))
                .json(&json!({ "repo": "sulion" })),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(refreshed["backfills_started"], 3, "{refreshed:#}");

    retrieval::run_indexer_once_for_tests(&h.state, 10)
        .await
        .unwrap();
    let indexed_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM retrieval_embeddings")
        .fetch_one(&pool)
        .await
        .unwrap();
    // assistant_text + tool_call. The exec_command tool_result is no longer
    // embedded (only failures and `agent` finals are kept among tool results).
    assert_eq!(indexed_count, 2);

    let hidden_tool_semantic: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[
                    ("q", "semantic retrieval"),
                    ("search_mode", "semantic"),
                    ("include", "tool_call"),
                ])
                .header("x-sulion-repo", "sulion"),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        hidden_tool_semantic["results"].as_array().unwrap().len(),
        0,
        "{hidden_tool_semantic:#}"
    );

    let low_value_tool_semantic: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[
                    ("q", "semantic retrieval"),
                    ("search_mode", "semantic"),
                    ("include", "tool_call"),
                    ("include_low_value", "true"),
                ])
                .header("x-sulion-repo", "sulion"),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let low_value_tool_results = low_value_tool_semantic["results"].as_array().unwrap();
    assert_eq!(
        low_value_tool_results.len(),
        1,
        "{low_value_tool_semantic:#}"
    );
    assert_eq!(low_value_tool_results[0]["source_kind"], "tool_call");
    assert_eq!(low_value_tool_results[0]["tool"]["name"], "exec_command");

    let status: serde_json::Value = h
        .auth(h.client.get(format!("{}/v1/index/status", h.base)))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["pending_sources"], 0, "{status:#}");

    sqlx::query(
        "UPDATE event_blocks \
            SET text = 'The semantic retrieval updated text should refresh stale embeddings.' \
          WHERE session_uuid = $1 AND byte_offset = 10 AND ord = 0",
    )
    .bind(session_uuid)
    .execute(&pool)
    .await
    .unwrap();

    retrieval::run_indexer_once_for_tests(&h.state, 10)
        .await
        .unwrap();
    let updated_hashes: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM retrieval_embeddings WHERE content_hash = $1")
            .bind(hash_text(
                "The semantic retrieval updated text should refresh stale embeddings.",
            ))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(updated_hashes, 1);

    let semantic: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[
                    ("q", "semantic retrieval"),
                    ("search_mode", "semantic"),
                    ("include", "assistant"),
                ])
                .header("x-sulion-repo", "sulion"),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let results = semantic["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{semantic:#}");
    assert_eq!(results[0]["agent_session_uuid"], session_uuid.to_string());
}

#[tokio::test]
async fn indexing_batch_rolls_back_embeddings_when_source_finalization_fails() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let embedding_url = start_embedding_server().await;
    let pool = fresh_pool().await;
    seed_retrieval_fixture(&pool).await;
    let h = Harness::new_with_config(
        pool.clone(),
        retrieval::RetrievalConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            db_url: String::new(),
            token: "test-token".to_string(),
            embedding_service_url: embedding_url,
            embedding_model: "test-embed".to_string(),
            embedding_dimensions: 3,
            embedding_batch_size: 8,
            embedding_max_chars: 6000,
            embedding_chunk_max: 10,
            semantic_min_score: 0.0,
            background_index_seconds: None,
        },
    )
    .await;

    h.auth(
        h.client
            .post(format!("{}/v1/reindex", h.base))
            .json(&json!({ "repo": "sulion" })),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap();

    // Force the final source-state update to fail after its embedding upsert.
    // The whole indexing batch must roll back, leaving no partial embeddings.
    sqlx::query(
        "ALTER TABLE retrieval_embedding_sources \
         ADD CONSTRAINT retrieval_embedding_sources_test_reject_indexed \
         CHECK (index_status <> 'indexed')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let result = retrieval::run_indexer_once_for_tests(&h.state, 10).await;
    sqlx::query(
        "ALTER TABLE retrieval_embedding_sources \
         DROP CONSTRAINT retrieval_embedding_sources_test_reject_indexed",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert!(result.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM retrieval_embeddings")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn reset_endpoint_wipes_index_and_reschedules_backfill() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let embedding_url = start_embedding_server().await;
    let pool = fresh_pool().await;
    seed_retrieval_fixture(&pool).await;
    let h = Harness::new_with_config(
        pool.clone(),
        retrieval::RetrievalConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            db_url: String::new(),
            token: "test-token".to_string(),
            embedding_service_url: embedding_url,
            embedding_model: "test-embed".to_string(),
            embedding_dimensions: 3,
            embedding_batch_size: 8,
            embedding_max_chars: 6000,
            embedding_chunk_max: 10,
            semantic_min_score: 0.0,
            background_index_seconds: None,
        },
    )
    .await;

    // Build an index: assistant_text + tool_call.
    h.auth(
        h.client
            .post(format!("{}/v1/reindex", h.base))
            .json(&json!({ "repo": "sulion" })),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap();
    retrieval::run_indexer_once_for_tests(&h.state, 10)
        .await
        .unwrap();
    let before = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM retrieval_embeddings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, 2);

    // Reset is refused without confirmation.
    let unconfirmed = h
        .auth(
            h.client
                .post(format!("{}/v1/index/reset", h.base))
                .json(&json!({})),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(unconfirmed.status(), StatusCode::BAD_REQUEST);

    // Confirmed reset wipes the index and reschedules a fresh backfill.
    let body: serde_json::Value = h
        .auth(
            h.client
                .post(format!("{}/v1/index/reset", h.base))
                .json(&json!({ "confirm": true })),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["embeddings_deleted"], 2, "{body:#}");
    assert_eq!(body["backfills_started"], 3, "{body:#}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM retrieval_embeddings")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );

    // The rescheduled backfill rebuilds under the same rules.
    retrieval::run_indexer_once_for_tests(&h.state, 10)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM retrieval_embeddings")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn repo_scope_does_not_narrow_to_current_agent_session_header() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    let evidence_session_uuid = seed_retrieval_fixture(&pool).await;
    let current_session_uuid = seed_empty_session(&pool, "sulion").await;
    let h = Harness::new(pool).await;

    let body: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/search", h.base))
                .query(&[
                    ("q", "retrieval api"),
                    ("search_mode", "lexical"),
                    ("limit", "5"),
                ])
                .header("x-sulion-repo", "sulion")
                .header(
                    "x-sulion-agent-session-id",
                    current_session_uuid.to_string(),
                ),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{body:#}");
    assert_eq!(
        results[0]["agent_session_uuid"],
        evidence_session_uuid.to_string()
    );
}

#[tokio::test]
async fn file_history_uses_existing_timeline_file_touches() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    seed_retrieval_fixture(&pool).await;
    let h = Harness::new(pool).await;

    let body: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/files/history", h.base))
                .query(&[
                    ("repo", "sulion"),
                    ("path", "backend/src/retrieval.rs"),
                    ("limit", "10"),
                ]),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let items = body.as_array().unwrap();
    assert_eq!(items.len(), 1, "{body:#}");
    assert_eq!(items[0]["is_write"], true);
}

async fn seed_retrieval_fixture(pool: &db::Pool) -> Uuid {
    seed_retrieval_fixture_for_repo(pool, "sulion").await
}

async fn seed_retrieval_fixture_for_repo(pool: &db::Pool, repo: &str) -> Uuid {
    let pty_id = Uuid::new_v4();
    let session_uuid = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state) \
         VALUES ($1, $2, '/repo', 'dead')",
    )
    .bind(pty_id)
    .bind(repo)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO claude_sessions (session_uuid, pty_session_id, agent, started_at) \
         VALUES ($1, $2, 'codex', $3)",
    )
    .bind(session_uuid)
    .bind(pty_id)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent_session_metadata (session_uuid, agent, model, cwd) \
         VALUES ($1, 'codex', 'test-model', $2)",
    )
    .bind(session_uuid)
    .bind(format!("/home/sulion/repos/{repo}"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO events \
            (session_uuid, byte_offset, timestamp, kind, payload, agent, speaker, content_kind, search_text) \
         VALUES ($1, 10, $2, 'message', $3, 'codex', 'assistant', 'text', 'retrieval api')",
    )
    .bind(session_uuid)
    .bind(now)
    .bind(json!({ "test": true }))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO event_blocks (session_uuid, byte_offset, ord, kind, text) \
         VALUES ($1, 10, 0, 'text', 'The semantic retrieval api should find assistant evidence by repo.')",
    )
    .bind(session_uuid)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO timeline_turns \
            (session_uuid, turn_id, turn_ord, preview, user_prompt_text, start_timestamp, end_timestamp, \
             duration_ms, event_count, operation_count, thinking_count, has_errors, markdown, turn_json, chunks_json) \
         VALUES ($1, 10, 0, 'retrieval work', 'build retrieval', $2, $2, 0, 1, 1, 0, false, \
                 'The retrieval api should find assistant evidence by repo.', $3, '[]'::jsonb)",
    )
    .bind(session_uuid)
    .bind(now)
    .bind(json!({ "id": 10 }))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO timeline_operations \
            (session_uuid, turn_id, operation_ord, pair_id, name, operation_type, operation_category, \
             input, result_content, result_payload, result_is_error, is_error, is_pending) \
         VALUES ($1, 10, 0, 'op-1', 'exec_command', 'exec_command', 'utility', \
                 $2, 'command output mentions retrieval api', $3, false, false, false)",
    )
    .bind(session_uuid)
    .bind(json!({ "cmd": "rg semantic retrieval api" }))
    .bind(json!({ "stdout": "command output mentions retrieval api" }))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO timeline_file_touches \
            (session_uuid, turn_id, touch_ord, operation_ord, repo_name, repo_rel_path, touch_kind, is_write) \
         VALUES ($1, 10, 0, 0, $2, 'backend/src/retrieval.rs', 'write', true)",
    )
    .bind(session_uuid)
    .bind(repo)
    .execute(pool)
    .await
    .unwrap();
    session_uuid
}

async fn seed_oversized_retrieval_text(pool: &db::Pool, session_uuid: Uuid) {
    let now = Utc::now();
    let oversized = format!("oversized lexical guard {}", "x".repeat(1_050_000));
    sqlx::query(
        "INSERT INTO events \
            (session_uuid, byte_offset, timestamp, kind, payload, agent, speaker, content_kind, search_text) \
         VALUES ($1, 20, $2, 'message', $3, 'codex', 'assistant', 'text', 'oversized lexical guard')",
    )
    .bind(session_uuid)
    .bind(now)
    .bind(json!({ "test": true, "oversized": true }))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO event_blocks (session_uuid, byte_offset, ord, kind, text) \
         VALUES ($1, 20, 0, 'text', $2)",
    )
    .bind(session_uuid)
    .bind(&oversized)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "UPDATE timeline_operations \
            SET result_content = $2, result_payload = NULL \
          WHERE session_uuid = $1 AND turn_id = 10 AND operation_ord = 0",
    )
    .bind(session_uuid)
    .bind("y".repeat(1_050_000))
    .execute(pool)
    .await
    .unwrap();
}

fn hash_text(text: &str) -> String {
    let hash = digest::digest(&digest::SHA256, text.as_bytes());
    hash.as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn seed_empty_session(pool: &db::Pool, repo: &str) -> Uuid {
    let pty_id = Uuid::new_v4();
    let session_uuid = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state) VALUES ($1, $2, '/repo', 'dead')",
    )
    .bind(pty_id)
    .bind(repo)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO claude_sessions (session_uuid, pty_session_id, agent, started_at) \
         VALUES ($1, $2, 'codex', $3)",
    )
    .bind(session_uuid)
    .bind(pty_id)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent_session_metadata (session_uuid, agent, model, cwd) \
         VALUES ($1, 'codex', 'test-model', $2)",
    )
    .bind(session_uuid)
    .bind(format!("/home/sulion/repos/{repo}"))
    .execute(pool)
    .await
    .unwrap();
    session_uuid
}
