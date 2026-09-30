#![cfg(feature = "integration-tests")]

use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde_json::json;
use sulion::db;
use sulion::repo_ci::GithubCi;
use sulion::repo_lifecycle::RepoLifecycleGate;
use sulion::repo_state::RepoStateManager;
use tokio::net::TcpListener;

async fn fresh_pool() -> db::Pool {
    let url = std::env::var("SULION_TEST_DB").expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    db::run_migrations(&pool).await.expect("migrate");
    sqlx::query("TRUNCATE repo_runtime_state, repo_dirty_paths RESTART IDENTITY CASCADE")
        .execute(&pool)
        .await
        .expect("truncate repo state");
    pool
}

type Requests = Arc<Mutex<Vec<String>>>;

/// Mock of `GET /repos/{owner}/{name}/actions/runs`: `acme/app` has an
/// in-progress run, `acme/private` is not visible anonymously, and
/// `acme/limited` reports an exhausted rate limit.
async fn runs(
    State(requests): State<Requests>,
    UrlPath((owner, name)): UrlPath<(String, String)>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let branch = query.get("branch").cloned().unwrap_or_default();
    requests
        .lock()
        .unwrap()
        .push(format!("{owner}/{name}@{branch}"));
    match name.as_str() {
        "app" => (
            StatusCode::OK,
            HeaderMap::new(),
            Json(json!({
                "total_count": 1,
                "workflow_runs": [{
                    "status": "in_progress",
                    "conclusion": null,
                    "html_url": "https://github.com/acme/app/actions/runs/7",
                    "updated_at": "2026-09-30T12:00:00Z"
                }]
            })),
        ),
        "limited" => {
            let mut headers = HeaderMap::new();
            headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
            headers.insert("x-ratelimit-reset", "4102444800".parse().unwrap());
            (
                StatusCode::FORBIDDEN,
                headers,
                Json(json!({"message": "rate limited"})),
            )
        }
        _ => (
            StatusCode::NOT_FOUND,
            HeaderMap::new(),
            Json(json!({"message": "Not Found"})),
        ),
    }
}

async fn mock_github() -> (String, Requests) {
    let requests: Requests = Arc::default();
    let router = Router::new()
        .route("/repos/:owner/:name/actions/runs", get(runs))
        .with_state(requests.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (format!("http://{addr}"), requests)
}

fn init_repo(path: &Path, origin: &str) {
    std::fs::create_dir_all(path).unwrap();
    for args in [
        &["init", "-b", "main"][..],
        &["config", "user.email", "sulion@example.invalid"],
        &["config", "user.name", "Sulion Test"],
        &["remote", "add", "origin", origin],
        &["commit", "--allow-empty", "-m", "initial"],
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
    }
}

type CiRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    DateTime<Utc>,
);

async fn ci_row(pool: &db::Pool, repo: &str) -> CiRow {
    sqlx::query_as(
        "SELECT origin_url, ci_state, ci_branch, ci_run_url, next_ci_at \
           FROM repo_runtime_state WHERE repo_name = $1",
    )
    .bind(repo)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn node_records_public_actions_status_and_honours_rate_limits() {
    let pool = fresh_pool().await;
    let tmp = tempfile::tempdir().unwrap();
    let repos_root = tmp.path().join("repos");
    init_repo(
        &repos_root.join("app"),
        "https://token:secret@github.com/acme/app.git",
    );
    init_repo(
        &repos_root.join("private"),
        "git@github.com:acme/private.git",
    );
    init_repo(&repos_root.join("local"), "/srv/git/local.git");
    init_repo(
        &repos_root.join("ylimited"),
        "git@github.com:acme/limited.git",
    );
    init_repo(&repos_root.join("zafter"), "git@github.com:acme/app.git");

    let (api_base, requests) = mock_github().await;
    let manager = RepoStateManager::with_github_ci(
        pool.clone(),
        repos_root,
        RepoLifecycleGate::default(),
        GithubCi::with_api_base(api_base),
    );
    manager.sync_repos_once().await.unwrap();
    // Due rows tie, so they reconcile in name order: `ylimited` hits the rate
    // limit after `private` is checked and before `zafter` is.
    for _ in 0..5 {
        manager.reconcile_due_once(1).await.unwrap();
    }

    let (origin, state, branch, url, next) = ci_row(&pool, "app").await;
    assert_eq!(origin.as_deref(), Some("https://github.com/acme/app.git"));
    assert_eq!(state.as_deref(), Some("in_progress"));
    assert_eq!(branch.as_deref(), Some("main"));
    assert_eq!(
        url.as_deref(),
        Some("https://github.com/acme/app/actions/runs/7")
    );
    let active = next - Utc::now();
    assert!(
        active > chrono::Duration::minutes(8) && active <= chrono::Duration::minutes(10),
        "{active}"
    );

    let (_, state, _, _, next) = ci_row(&pool, "private").await;
    assert_eq!(state, None);
    assert!(next - Utc::now() > chrono::Duration::hours(23));

    let (_, state, branch, _, _) = ci_row(&pool, "local").await;
    assert_eq!((state, branch), (None, None));

    let reset: DateTime<Utc> = "2100-01-01T00:00:00Z".parse().unwrap();
    assert_eq!(ci_row(&pool, "ylimited").await.4, reset);
    assert_eq!(ci_row(&pool, "zafter").await.4, reset);

    let mut seen = requests.lock().unwrap().clone();
    seen.sort();
    assert_eq!(
        seen,
        vec!["acme/app@main", "acme/limited@main", "acme/private@main"],
        "non-GitHub origins and paused checks make no request"
    );
}
