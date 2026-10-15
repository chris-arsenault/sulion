#![cfg(feature = "integration-tests")]

use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sulion::repo_ci::{reconcile_repo_dora, GithubAuth, GithubCi};
use sulion::{db, dora};
use tokio::net::TcpListener;

async fn fresh_pool() -> db::Pool {
    let pool = db::connect(&std::env::var("SULION_TEST_DB").unwrap())
        .await
        .unwrap();
    db::run_migrations(&pool).await.unwrap();
    sqlx::query("TRUNCATE dora_deployments, dora_commit_deployments, dora_run_checks, repo_runtime_state CASCADE")
        .execute(&pool).await.unwrap();
    pool
}

fn git(path: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[derive(Clone)]
struct Fixture {
    runs: Value,
    second_page: Value,
    jobs: Arc<Mutex<Vec<String>>>,
    at: DateTime<Utc>,
}

async fn runs(
    State(fixture): State<Fixture>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer read-only-fixture")
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message": "Bad credentials"})),
        );
    }
    assert_eq!(query["branch"], "main");
    if query["page"] == "1" {
        (StatusCode::OK, Json(fixture.runs))
    } else {
        (StatusCode::OK, Json(fixture.second_page))
    }
}

async fn jobs(
    State(fixture): State<Fixture>,
    UrlPath((run, attempt)): UrlPath<(i64, i32)>,
) -> Json<Value> {
    fixture
        .jobs
        .lock()
        .unwrap()
        .push(format!("{run}:{attempt}"));
    let step = if run == 3 {
        "Test rust"
    } else {
        "Run chris-arsenault/ahara/.github/actions/deploy-truenas@main"
    };
    Json(json!({"jobs": [{"steps": [
        {"name": step, "conclusion": "success", "completed_at": fixture.at + chrono::Duration::hours(run)},
        {"name": "Upload report", "conclusion": "failure", "completed_at": fixture.at + chrono::Duration::hours(run + 1)}
    ]}]}))
}

#[tokio::test]
async fn window_excludes_old_and_future_deployments_and_keeps_open_recovery_unknown() {
    let pool = fresh_pool().await;
    sqlx::query("INSERT INTO repo_runtime_state(repo_name, path, exists, dora_checked_at) VALUES ('window', '/unmounted', TRUE, NOW())")
        .execute(&pool).await.unwrap();
    let id = uuid::Uuid::new_v4();
    let deployed_at = Utc::now() - chrono::Duration::hours(3);
    sqlx::query("INSERT INTO dora_deployments(id, repo_name, source_key, revision, deployed_at, source_url) VALUES ($1, 'window', 'current', 'current', $2, 'https://github.com/acme/app/actions/runs/1')")
        .bind(id).bind(deployed_at).execute(&pool).await.unwrap();
    for (key, days) in [("old", -31), ("future", 1)] {
        sqlx::query("INSERT INTO dora_deployments(id, repo_name, source_key, revision, deployed_at, source_url) VALUES ($1, 'window', $2, $2, $3, 'https://github.com/acme/app/actions/runs/1')")
            .bind(uuid::Uuid::new_v4()).bind(key).bind(Utc::now() + chrono::Duration::days(days)).execute(&pool).await.unwrap();
    }
    let initial = dora::metrics(&pool).await.unwrap();
    assert_eq!(initial.repos[0].deployments, 1);
    assert_eq!(initial.repos[0].change_failure_rate, None);
    assert_eq!(initial.repos[0].recent_deployments.len(), 1);
    let mut input = dora::DeploymentOutcome {
        outcome: "failed".into(),
        failed_at: Some(deployed_at + chrono::Duration::hours(1)),
        recovered_at: None,
        rework: Some(false),
    };
    dora::update_outcome(&pool, id, &input).await.unwrap();
    let open = dora::metrics(&pool).await.unwrap();
    assert_eq!(open.repos[0].change_failure_rate, Some(1.0));
    assert_eq!(open.repos[0].rework_rate, Some(0.0));
    assert_eq!(open.repos[0].recovery_time_hours_p50, None);
    assert_eq!(open.repos[0].open_failures, 1);
    input.recovered_at = Some(deployed_at + chrono::Duration::hours(2));
    dora::update_outcome(&pool, id, &input).await.unwrap();
    let recovered = dora::metrics(&pool).await.unwrap();
    assert_eq!(recovered.repos[0].recovery_time_hours_p50, Some(1.0));
    assert_eq!(recovered.repos[0].open_failures, 0);
    input.recovered_at = Some(deployed_at);
    assert!(dora::update_outcome(&pool, id, &input).await.is_err());
    assert_eq!(
        dora::metrics(&pool).await.unwrap().repos[0].recovery_time_hours_p50,
        Some(1.0)
    );
}

#[tokio::test]
async fn collects_actual_deploy_steps_idempotently_and_measures_first_deployed_changes() {
    let pool = fresh_pool().await;
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "-b", "main"]);
    git(
        temp.path(),
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(temp.path(), &["config", "user.name", "Fixture"]);
    git(temp.path(), &["commit", "--allow-empty", "-m", "baseline"]);
    let first = git(temp.path(), &["rev-parse", "HEAD"]);
    git(temp.path(), &["commit", "--allow-empty", "-m", "feature"]);
    let second = git(temp.path(), &["rev-parse", "HEAD"]);
    let now = Utc::now();
    // Set real Git commit timestamps before the mocked production deployments.
    // This updates generated test Git objects, not project source.
    let at = now - chrono::Duration::hours(8);
    let tree = git(temp.path(), &["rev-parse", "HEAD^{tree}"]);
    let stamp = (at - chrono::Duration::hours(2)).to_rfc3339();
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(temp.path())
        .args(["commit-tree", &tree, "-p", &first, "-m", "feature"])
        .env("GIT_AUTHOR_DATE", &stamp)
        .env("GIT_COMMITTER_DATE", &stamp)
        .output()
        .unwrap();
    assert!(output.status.success());
    let changed = String::from_utf8(output.stdout).unwrap().trim().to_string();
    assert_ne!(changed, second);
    let run = |id: i64, sha: &str| {
        json!({
            "id": id, "run_attempt": 1, "status": "completed", "event": "push",
            "head_branch": "main", "head_sha": sha,
            "html_url": format!("https://github.com/acme/app/actions/runs/{id}")
        })
    };
    let mut history = vec![run(3, &changed), run(2, &changed), run(1, &first)];
    // A full first page forces the collector through its pagination path.
    for id in 5..102 {
        let mut skipped = run(id, &changed);
        skipped["event"] = json!("pull_request");
        history.push(skipped);
    }
    let fixture = Fixture {
        runs: json!({"total_count": 101, "workflow_runs": history}),
        second_page: json!({"total_count": 101, "workflow_runs": [run(4, &changed)]}),
        jobs: Arc::default(),
        at,
    };
    let seen = fixture.jobs.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new()
        .route("/repos/acme/app/actions/runs", get(runs))
        .route(
            "/repos/acme/app/actions/runs/:run/attempts/:attempt/jobs",
            get(jobs),
        )
        .with_state(fixture);
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    sqlx::query("INSERT INTO repo_runtime_state(repo_name, path, exists, origin_url) VALUES ('app', $1, TRUE, 'https://github.com/acme/app.git')")
        .bind(temp.path().to_str().unwrap()).execute(&pool).await.unwrap();
    let ci = GithubCi::new(
        format!("http://{addr}"),
        GithubAuth::Static("read-only-fixture".into()),
    );
    reconcile_repo_dora(&pool, &ci, "app", temp.path())
        .await
        .unwrap();
    reconcile_repo_dora(&pool, &ci, "app", temp.path())
        .await
        .unwrap();
    assert_eq!(
        seen.lock().unwrap().len(),
        4,
        "completed attempts are not fetched again"
    );
    let metrics = dora::metrics(&pool).await.unwrap();
    let repo = &metrics.repos[0];
    assert_eq!(
        repo.deployments, 3,
        "test-only/PR runs are excluded; second-page redeployment is included"
    );
    assert_eq!(repo.deployments_per_day, Some(3.0 / 30.0));
    assert_eq!(repo.lead_time_samples, 1, "baseline commits are excluded");
    assert!((repo.change_lead_time_hours_p50.unwrap() - 4.0).abs() < 0.001);
    assert_eq!(repo.change_failure_rate, None);
    assert_eq!(repo.rework_rate, None);
    let failed = &repo.recent_deployments[0];
    dora::update_outcome(
        &pool,
        failed.id,
        &dora::DeploymentOutcome {
            outcome: "failed".into(),
            failed_at: Some(failed.deployed_at + chrono::Duration::minutes(10)),
            recovered_at: Some(failed.deployed_at + chrono::Duration::minutes(40)),
            rework: Some(true),
        },
    )
    .await
    .unwrap();
    for healthy in &repo.recent_deployments[1..] {
        dora::update_outcome(
            &pool,
            healthy.id,
            &dora::DeploymentOutcome {
                outcome: "healthy".into(),
                failed_at: None,
                recovered_at: None,
                rework: Some(false),
            },
        )
        .await
        .unwrap();
    }
    // Collection retains explicit production outcomes.
    reconcile_repo_dora(&pool, &ci, "app", temp.path())
        .await
        .unwrap();
    let metrics = dora::metrics(&pool).await.unwrap();
    let repo = &metrics.repos[0];
    assert_eq!(repo.change_failure_rate, Some(1.0 / 3.0));
    assert_eq!(repo.rework_rate, Some(1.0 / 3.0));
    assert_eq!(repo.recovery_time_hours_p50, Some(0.5));
    assert_eq!(repo.open_failures, 0);
    // A redeployment of the same commit cannot create new lead-time samples.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dora_commit_deployments WHERE repo_name = 'app' AND NOT baseline",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let rejected = GithubCi::new(
        format!("http://{addr}"),
        GithubAuth::Static("rejected-fixture".into()),
    );
    let error = reconcile_repo_dora(&pool, &rejected, "app", temp.path())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("401"));
    let retained = dora::metrics(&pool).await.unwrap();
    assert_eq!(retained.repos[0].deployments, 3);
    assert!(retained.repos[0]
        .collection_error
        .as_ref()
        .unwrap()
        .contains("401"));
    server.abort();
}
