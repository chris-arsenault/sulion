#![cfg(feature = "integration-tests")]

//! Code-intelligence service integration tests: real Postgres, real axum stack,
//! and temporary source roots. Query endpoints must not perform indexing work.

use reqwest::StatusCode;
use sqlx::Row;
use sulion::{
    code_intel,
    code_intel::indexer::{IndexOptions, IndexTrigger},
    db,
};
use tokio::net::TcpListener;

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    db::run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE code_imports, code_references, code_symbols, code_files, \
         code_index_jobs, code_roots RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate code-intel test tables");
    pool
}

struct Harness {
    base: String,
    client: reqwest::Client,
}

impl Harness {
    async fn new(pool: db::Pool, allowed_root: std::path::PathBuf) -> Self {
        let config = code_intel::CodeIntelConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            db_url: String::new(),
            token: "test-token".to_string(),
            allowed_roots: vec![allowed_root],
        };
        let state = code_intel::CodeIntelState::from_pool_for_tests(pool, config);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = code_intel::app(state);
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            base: format!("http://{addr}"),
            client: reqwest::Client::new(),
        }
    }

    fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.bearer_auth("test-token")
    }
}

#[tokio::test]
async fn find_reads_existing_index_without_creating_query_job() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    let temp = tempfile::tempdir().unwrap();
    let repos = temp.path().join("repos");
    let repo = repos.join("sulion");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("src/lib.rs"),
        "pub struct RetrievalState;\nfn use_state(_: RetrievalState) {}\n",
    )
    .unwrap();
    let h = Harness::new(pool.clone(), repos).await;

    let refresh: serde_json::Value = h
        .auth(h.client.post(format!("{}/v1/refresh", h.base)).query(&[
            ("cwd", repo.to_string_lossy()),
            ("path", "src/lib.rs".into()),
        ]))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(refresh["stats"]["files_marked_pending"], 1);
    assert_eq!(refresh["freshness"], "stale");

    code_intel::indexer::index_allowed_roots(
        &pool,
        &[temp.path().join("repos")],
        &IndexOptions {
            trigger: IndexTrigger::Background,
            ..IndexOptions::default()
        },
    )
    .await
    .unwrap();

    let body: serde_json::Value = h
        .auth(h.client.get(format!("{}/v1/find", h.base)).query(&[
            ("cwd", repo.to_string_lossy()),
            ("q", "RetrievalState".into()),
        ]))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["command"], "find");
    assert_eq!(body["results"][0]["name"], "RetrievalState");

    let query_jobs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM code_index_jobs WHERE trigger = 'query'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(query_jobs, 0);
}

#[tokio::test]
async fn refresh_marks_pending_without_creating_index_job() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    let temp = tempfile::tempdir().unwrap();
    let repos = temp.path().join("repos");
    let repo = repos.join("sulion");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub struct PendingRefresh;\n").unwrap();
    let h = Harness::new(pool.clone(), repos).await;

    let body: serde_json::Value = h
        .auth(h.client.post(format!("{}/v1/refresh", h.base)).query(&[
            ("cwd", repo.to_string_lossy()),
            ("path", "src/lib.rs".into()),
        ]))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["command"], "refresh");
    assert_eq!(body["stats"]["files_seen"], 1);
    assert_eq!(body["stats"]["files_marked_pending"], 1);
    assert_eq!(body["stats"]["files_deleted"], 0);
    let parse_status: String =
        sqlx::query_scalar("SELECT parse_status FROM code_files WHERE path = 'src/lib.rs'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(parse_status, "pending");
    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM code_index_jobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
}

#[tokio::test]
async fn index_status_reports_pending_backlog() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    let temp = tempfile::tempdir().unwrap();
    let repos = temp.path().join("repos");
    let repo = repos.join("sulion");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub struct PendingStatus;\n").unwrap();
    let h = Harness::new(pool.clone(), repos).await;

    h.auth(h.client.post(format!("{}/v1/refresh", h.base)).query(&[
        ("cwd", repo.to_string_lossy()),
        ("path", "src/lib.rs".into()),
    ]))
    .send()
    .await
    .unwrap();

    let body: serde_json::Value = h
        .auth(
            h.client
                .get(format!("{}/v1/index/status", h.base))
                .query(&[("cwd", repo.to_string_lossy())]),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["command"], "index_status");
    assert_eq!(body["freshness"], "stale");
    assert_eq!(body["index"]["pending_file_count"], 1);
    assert!(body["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().unwrap().contains("pending indexing")));
}

#[tokio::test]
async fn startup_indexer_discovers_roots_without_manual_refresh() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    let temp = tempfile::tempdir().unwrap();
    let repos = temp.path().join("repos");
    let repo = repos.join("sulion");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub struct StartupIndexed;\n").unwrap();
    let config = code_intel::CodeIntelConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        db_url: String::new(),
        token: "test-token".to_string(),
        allowed_roots: vec![repos],
    };
    let state = code_intel::CodeIntelState::from_pool_for_tests(pool.clone(), config);

    let stats = code_intel::indexer::run_startup_indexer_once(state)
        .await
        .unwrap();
    assert_eq!(stats.files_seen, 1);
    assert_eq!(stats.files_indexed, 1);

    let symbol_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM code_symbols WHERE name = 'StartupIndexed'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(symbol_count, 1);
    let startup_jobs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM code_index_jobs WHERE trigger = 'startup'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(startup_jobs, 1);
}

#[tokio::test]
async fn find_on_missing_index_returns_stale_empty_response() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    let temp = tempfile::tempdir().unwrap();
    let repos = temp.path().join("repos");
    let repo = repos.join("sulion");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub struct MissingIndex;\n").unwrap();
    let h = Harness::new(pool.clone(), repos).await;

    let response = h
        .auth(h.client.get(format!("{}/v1/find", h.base)).query(&[
            ("cwd", repo.to_string_lossy()),
            ("q", "MissingIndex".into()),
        ]))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["freshness"], "stale");
    assert_eq!(body["results"].as_array().unwrap().len(), 0);
    assert!(body["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning
            .as_str()
            .unwrap()
            .contains("run sulion-code refresh")));

    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM code_index_jobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
}

#[tokio::test]
async fn startup_reconciles_orphaned_running_jobs() {
    let Some(_) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = fresh_pool().await;
    let root_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO code_roots (root_kind, name, path, deleted_at) \
         VALUES ('repo', 'sulion', '/tmp/sulion', NULL) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO code_index_jobs (root_id, status, trigger, started_at) \
         VALUES ($1, 'running', 'manual', NOW())",
    )
    .bind(root_id)
    .execute(&pool)
    .await
    .unwrap();

    code_intel::indexer::cancel_orphaned_running_jobs(&pool)
        .await
        .unwrap();

    let row = sqlx::query("SELECT status, error FROM code_index_jobs WHERE root_id = $1")
        .bind(root_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let status: String = row.get("status");
    let error: String = row.get("error");
    assert_eq!(status, "cancelled");
    assert!(error.contains("service restarted"));
}
