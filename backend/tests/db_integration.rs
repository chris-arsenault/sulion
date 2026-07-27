#![cfg(feature = "integration-tests")]

//! Integration tests that require a live Postgres.
//!
//! Run via `make test-rust-integration`, or point `SULION_TEST_DB`
//! at an existing Postgres and invoke
//! `cargo test --release --features integration-tests --test db_integration -- --test-threads=1`.

use sulion::db;

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn reset_schema(pool: &db::Pool) {
    // Migration tests need a true clean-room schema so newly added tables
    // cannot survive between runs and mask drift in the reset list itself.
    sqlx::query("DROP SCHEMA public CASCADE")
        .execute(pool)
        .await
        .expect("drop schema failed");
    sqlx::query("CREATE SCHEMA public")
        .execute(pool)
        .await
        .expect("create schema failed");
}

#[tokio::test]
async fn migrations_apply_on_empty_db() {
    let Some(url) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = db::connect(&url).await.expect("connect");
    reset_schema(&pool).await;
    db::run_migrations(&pool).await.expect("migrate");
    db::ping(&pool).await.expect("ping");

    let tables: Vec<(String,)> = sqlx::query_as(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_type = 'BASE TABLE' \
         ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .expect("list");
    let names: Vec<String> = tables.into_iter().map(|(n,)| n).collect();
    for expected in [
        "claude_sessions",
        "code_files",
        "code_imports",
        "code_index_jobs",
        "code_references",
        "code_roots",
        "code_symbols",
        "dev_node_boots",
        "dev_node_credentials",
        "dev_node_enrollment_tokens",
        "dev_node_operations",
        "dev_nodes",
        "events",
        "event_blocks",
        "ingester_state",
        "pty_sessions",
        "repos",
        "retrieval_embedding_backfills",
        "retrieval_embedding_sources",
        "retrieval_embeddings",
        "timeline_activity_signals",
        "timeline_file_touches",
        "timeline_operations",
        "timeline_turns",
        "tool_category_rules",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "table {expected} missing; got {names:?}"
        );
    }
}

#[tokio::test]
async fn migrations_are_idempotent() {
    let Some(url) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = db::connect(&url).await.expect("connect");
    reset_schema(&pool).await;
    db::run_migrations(&pool).await.expect("first migrate");
    db::run_migrations(&pool).await.expect("second migrate");
}

#[tokio::test]
async fn migration_wait_returns_after_backend_migrations_apply() {
    let Some(url) = test_db_url() else {
        eprintln!("skipping: SULION_TEST_DB not set");
        return;
    };
    let pool = db::connect(&url).await.expect("connect");
    reset_schema(&pool).await;
    db::run_migrations(&pool).await.expect("migrate");

    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        db::wait_for_migrations(&pool, "test-service"),
    )
    .await
    .expect("wait timed out")
    .expect("wait failed");
}
