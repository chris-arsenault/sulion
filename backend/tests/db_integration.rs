//! Integration tests that require a live Postgres.
//!
//! Opt in with: `SHUTTLECRAFT_TEST_DB=postgres://... cargo test -- --ignored`.
//! Skipped by default to keep `make ci` DB-free.

use shuttlecraft::db;

fn test_db_url() -> Option<String> {
    std::env::var("SHUTTLECRAFT_TEST_DB").ok()
}

async fn reset_schema(pool: &db::Pool) {
    sqlx::query(
        "DROP TABLE IF EXISTS ingester_state, events, claude_sessions, pty_sessions, repos, _sqlx_migrations CASCADE",
    )
    .execute(pool)
    .await
    .expect("reset failed");
}

#[tokio::test]
#[ignore]
async fn migrations_apply_on_empty_db() {
    let Some(url) = test_db_url() else {
        eprintln!("skipping: SHUTTLECRAFT_TEST_DB not set");
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
        "events",
        "ingester_state",
        "pty_sessions",
        "repos",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "table {expected} missing; got {names:?}"
        );
    }
}

#[tokio::test]
#[ignore]
async fn migrations_are_idempotent() {
    let Some(url) = test_db_url() else {
        eprintln!("skipping: SHUTTLECRAFT_TEST_DB not set");
        return;
    };
    let pool = db::connect(&url).await.expect("connect");
    reset_schema(&pool).await;
    db::run_migrations(&pool).await.expect("first migrate");
    db::run_migrations(&pool).await.expect("second migrate");
}
