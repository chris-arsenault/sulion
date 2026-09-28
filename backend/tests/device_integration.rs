#![cfg(feature = "integration-tests")]

//! Regression guard for the retired device authentication surface.
use axum::{body::Body, http::Request};
use sulion::{app, db};
use tower::ServiceExt;

mod common;

#[tokio::test]
async fn retired_routes_reject_even_when_credentials_are_present() {
    let pool = db::connect(&std::env::var("SULION_TEST_DB").expect("isolated test database"))
        .await
        .unwrap();
    db::run_migrations(&pool).await.unwrap();
    let absent: bool = sqlx::query_scalar(
        "SELECT to_regclass('device_tokens') IS NULL AND to_regclass('device_pairings') IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(absent);

    // Exercise retirement with existing rows too, without changing migration history.
    let mut tx = pool.begin().await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0052_device_pairing.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_tokens (token_hash, user_sub, client) VALUES ('old-hash', 'user', 'unused')")
        .execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0097_retire_device_access.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let absent: bool = sqlx::query_scalar("SELECT to_regclass('device_tokens') IS NULL")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(absent);
    tx.rollback().await.unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let (state, _runtime) = common::state_with_loopback_node(
        pool,
        tmp.path(),
        &tmp.path().join(".workspaces"),
        &tmp.path().join(".library"),
    )
    .await;
    let router = app(state);
    for (method, path) in [
        ("POST", "/api/devices/pair"),
        ("POST", "/api/devices/pair/token"),
        ("POST", "/api/devices/pair/approve"),
        ("POST", "/api/repos/atlas/ingest?path=file"),
        ("GET", "/api/repos/atlas/raw?path=file"),
    ] {
        for credential in [None, Some("Bearer retired-device-token")] {
            let mut req = Request::builder().method(method).uri(path);
            if let Some(value) = credential {
                req = req.header("authorization", value);
            }
            let response = router
                .clone()
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), 404, "{method} {path}");
        }
    }
}
