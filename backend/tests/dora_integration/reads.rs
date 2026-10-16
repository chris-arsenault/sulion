use chrono::{Duration, Utc};
use sqlx::Row;
use sulion::dora::{self, DeploymentQuery};
use uuid::Uuid;

#[tokio::test]
async fn fifty_repo_rollup_stays_small_and_deployment_pages_use_stable_cursors() {
    let pool = super::fresh_pool().await;
    sqlx::raw_sql(include_str!("scale.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let metrics = dora::metrics(&pool).await.unwrap();
    let elapsed = started.elapsed();
    assert_eq!(metrics.repos.len(), 50);
    assert_eq!(metrics.summary.repository_count, 50);
    assert_eq!(metrics.summary.collected_repository_count, 50);
    assert_eq!(metrics.summary.deployments, 10_000);
    assert_eq!(metrics.summary.lead_time_samples, 50_000);
    assert_eq!(metrics.summary.change_lead_time_hours_p50, Some(2.0));
    assert_eq!(metrics.summary.change_failure_rate, Some(0.0));
    assert!(metrics
        .repos
        .iter()
        .all(|repo| repo.recent_deployments.is_empty()));
    let payload = serde_json::to_vec(&metrics).unwrap();
    assert!(
        payload.len() < 50_000,
        "summary payload unexpectedly grew: {} bytes",
        payload.len()
    );
    eprintln!(
        "DORA: 50 repos, 60000 deployments, 300000 commits; summary {:?}, {} bytes",
        elapsed,
        payload.len()
    );
    let first = dora::list_deployments(&pool, &DeploymentQuery::default())
        .await
        .unwrap();
    assert_eq!(first.deployments.len(), 25);
    let cursor = first.next_cursor.unwrap();
    let second = dora::list_deployments(
        &pool,
        &DeploymentQuery {
            before: Some(cursor.deployed_at),
            before_id: Some(cursor.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(second.deployments.len(), 25);
    assert!(
        second
            .deployments
            .iter()
            .all(|item| !first.deployments.iter().any(|prior| prior.id == item.id)),
        "equal timestamps must not duplicate rows across pages"
    );
    assert!(first
        .deployments
        .iter()
        .chain(&second.deployments)
        .all(|item| item.deployed_at >= Utc::now() - Duration::days(30)));
    let scoped = dora::list_deployments(
        &pool,
        &DeploymentQuery {
            repo: Some("scale-1".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(scoped.deployments.len(), 25);
    assert!(scoped
        .deployments
        .iter()
        .all(|item| item.repo_name == "scale-1"));
    assert!(dora::list_deployments(
        &pool,
        &DeploymentQuery {
            before: Some(cursor.deployed_at),
            ..Default::default()
        }
    )
    .await
    .is_err());
    let plan = sqlx::query("EXPLAIN (FORMAT JSON) SELECT committed_at FROM dora_commit_deployments WHERE deployment_id = $1 AND NOT baseline AND committed_at <= $2")
        .bind(first.deployments[0].id).bind(first.deployments[0].deployed_at).fetch_one(&pool).await.unwrap();
    let plan: serde_json::Value = plan.get(0);
    assert!(
        plan.to_string()
            .contains("dora_commits_deployment_time_idx"),
        "commit lookup should use its covering index: {plan}"
    );
}

#[tokio::test]
async fn portfolio_medians_use_all_samples_and_empty_portfolio_remains_unknown() {
    let pool = super::fresh_pool().await;
    let empty = dora::metrics(&pool).await.unwrap();
    assert_eq!(empty.summary.repository_count, 0);
    assert_eq!(empty.summary.deployments_per_day, None);
    assert!(empty.repos.is_empty());
    for (repo, changes, hours) in [("many", 9, 1), ("few", 1, 10)] {
        sqlx::query("INSERT INTO repo_runtime_state(repo_name, path, exists, dora_checked_at) VALUES ($1, $1, TRUE, NOW())")
            .bind(repo).execute(&pool).await.unwrap();
        let id = Uuid::new_v4();
        let deployed = Utc::now() - Duration::hours(1);
        sqlx::query("INSERT INTO dora_deployments(id, repo_name, source_key, revision, deployed_at, source_url, outcome, rework) VALUES ($1, $2, 'fixture', 'fixture', $3, 'https://github.com/fixture/app/actions/runs/1', 'healthy', FALSE)")
            .bind(id).bind(repo).bind(deployed).execute(&pool).await.unwrap();
        for n in 0..changes {
            sqlx::query("INSERT INTO dora_commit_deployments(repo_name, revision, deployment_id, committed_at, baseline) VALUES ($1, $2, $3, $4, FALSE)")
                .bind(repo).bind(format!("{n}")).bind(id).bind(deployed - Duration::hours(hours)).execute(&pool).await.unwrap();
        }
    }
    let metrics = dora::metrics(&pool).await.unwrap();
    assert_eq!(metrics.summary.change_lead_time_hours_p50, Some(1.0));
    assert_eq!(metrics.summary.lead_time_samples, 10);
    assert_eq!(metrics.summary.deployments, 2);
    assert_eq!(
        metrics
            .repos
            .iter()
            .find(|repo| repo.repo == "few")
            .unwrap()
            .change_lead_time_hours_p50,
        Some(10.0)
    );
}
