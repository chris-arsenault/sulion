//! Node-side collection for the known Ahara production workflow on main.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use uuid::Uuid;

use super::{github_repo, rate_limit_reset, GithubCi};
use crate::db::Pool;

#[derive(Deserialize)]
struct Runs {
    total_count: usize,
    workflow_runs: Vec<Run>,
}

#[derive(Deserialize)]
struct Run {
    id: i64,
    run_attempt: i32,
    status: String,
    event: String,
    head_branch: Option<String>,
    head_sha: String,
    html_url: String,
}

#[derive(Deserialize)]
struct Jobs {
    jobs: Vec<Job>,
}

#[derive(Deserialize)]
struct Job {
    steps: Vec<Step>,
}

#[derive(Deserialize)]
struct Step {
    name: String,
    conclusion: Option<String>,
    completed_at: Option<DateTime<Utc>>,
}

fn truenas_step(name: &str) -> bool {
    name == "Deploy to TrueNAS"
        || name == "Deploy to Komodo"
        || name.starts_with("Run chris-arsenault/ahara/.github/actions/deploy-truenas@")
}

fn production_step(name: &str) -> bool {
    truenas_step(name) || name == "Terraform deploy"
}

fn deployed_at(jobs: &[Job]) -> Option<DateTime<Utc>> {
    // A TrueNAS application deployment takes precedence over its preparatory
    // Terraform apply. Failed/skipped tests never become deployments.
    let truenas_attempted = jobs
        .iter()
        .flat_map(|job| &job.steps)
        .any(|step| truenas_step(&step.name) && step.conclusion.as_deref() != Some("skipped"));
    jobs.iter()
        .flat_map(|job| &job.steps)
        .filter(|step| {
            if truenas_attempted {
                truenas_step(&step.name)
            } else {
                production_step(&step.name)
            }
        })
        .filter(|step| step.conclusion.as_deref() == Some("success"))
        .filter_map(|step| step.completed_at)
        .max()
}

async fn get<T: DeserializeOwned>(ci: &GithubCi, path: &str, token: &str) -> anyhow::Result<T> {
    if ci.paused_until().is_some() {
        anyhow::bail!("GitHub rate limit pauses deployment collection");
    }
    let response = ci
        .http
        .get(format!("{}{path}", ci.api_base))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .context("request deployment evidence")?;
    let status = response.status();
    if let Some(until) = rate_limit_reset(status, response.headers()) {
        ci.pause(until);
        anyhow::bail!("GitHub rate limit pauses deployment collection until {until}");
    }
    if !status.is_success() {
        anyhow::bail!("GitHub deployment evidence returned {status}");
    }
    response.json().await.context("decode deployment evidence")
}

/// Shares CI authentication and rate-limit state, but has its own durable
/// half-hour schedule. A failed collection keeps previously collected evidence.
pub async fn reconcile(pool: &Pool, ci: &GithubCi, repo: &str, path: &Path) -> anyhow::Result<()> {
    let origin: Option<String> =
        sqlx::query_scalar("SELECT origin_url FROM repo_runtime_state WHERE repo_name = $1")
            .bind(repo)
            .fetch_optional(pool)
            .await?
            .flatten();
    let target = origin.as_deref().and_then(github_repo);
    let result = match target {
        Some((owner, name)) => collect(pool, ci, repo, path, &owner, &name).await,
        None => Err(anyhow::anyhow!("No GitHub production deployment source.")),
    };
    let (cadence, error) = match &result {
        Ok(warning) => (1800, warning.clone()),
        Err(error) => (3600, Some(format!("{error:#}"))),
    };
    sqlx::query(
        "UPDATE repo_runtime_state SET dora_error = $2, next_dora_at = $3 WHERE repo_name = $1",
    )
    .bind(repo)
    .bind(error)
    .bind(
        ci.paused_until()
            .unwrap_or_else(|| Utc::now() + chrono::Duration::seconds(cadence)),
    )
    .execute(pool)
    .await?;
    result.map(|_| ())
}

async fn collect(
    pool: &Pool,
    ci: &GithubCi,
    repo: &str,
    path: &Path,
    owner: &str,
    name: &str,
) -> anyhow::Result<Option<String>> {
    if ci.paused_until().is_some() {
        anyhow::bail!("GitHub rate limit pauses deployment collection");
    }
    let token = ci
        .token()
        .await
        .context("Deployment history needs the broker's GH_TOKEN grant for gh.")?;
    let now = Utc::now();
    let since = now - chrono::Duration::days(crate::dora::WINDOW_DAYS);
    // Include runs started before the window whose deployment finishes in it.
    let created = (since - chrono::Duration::days(1)).format("%Y-%m-%dT%H:%M:%SZ");
    let prefix = format!("/repos/{owner}/{name}/actions");
    let mut runs = Vec::new();
    for page in 1..=10 {
        let body: Runs = get(
            ci,
            &format!("{prefix}/runs?branch=main&created=%3E%3D{created}&per_page=100&page={page}"),
            &token,
        )
        .await?;
        anyhow::ensure!(body.total_count <= 1000, "More than 1000 Actions runs in the collection window; deployment coverage is incomplete.");
        let done = body.workflow_runs.len() < 100;
        runs.extend(body.workflow_runs);
        if done {
            break;
        }
    }
    let mut recognized = false;
    for run in runs {
        if run.head_branch.as_deref() != Some("main")
            || !matches!(run.event.as_str(), "push" | "workflow_dispatch")
        {
            continue;
        }
        anyhow::ensure!(
            run.head_sha.len() == 40 && run.head_sha.bytes().all(|c| c.is_ascii_hexdigit()),
            "Invalid deployment revision"
        );
        anyhow::ensure!(
            run.html_url
                .starts_with(&format!("https://github.com/{owner}/{name}/actions/runs/")),
            "Invalid deployment evidence URL"
        );
        for attempt in 1..=run.run_attempt {
            let key = format!("{owner}/{name}:{}:{attempt}", run.id);
            let checked: Option<bool> = sqlx::query_scalar("SELECT production_source FROM dora_run_checks WHERE repo_name = $1 AND source_key = $2")
                .bind(repo).bind(&key).fetch_optional(pool).await?;
            if let Some(supported) = checked {
                recognized |= supported;
                continue;
            }
            let mut jobs = Vec::new();
            for page in 1.. {
                let body: Jobs = get(
                    ci,
                    &format!(
                        "{prefix}/runs/{}/attempts/{attempt}/jobs?per_page=100&page={page}",
                        run.id
                    ),
                    &token,
                )
                .await?;
                let done = body.jobs.len() < 100;
                jobs.extend(body.jobs);
                if done {
                    break;
                }
            }
            let supported = jobs
                .iter()
                .flat_map(|job| &job.steps)
                .any(|step| production_step(&step.name));
            recognized |= supported;
            if let Some(at) = deployed_at(&jobs).filter(|at| *at <= now) {
                let id = Uuid::new_v5(
                    &Uuid::NAMESPACE_URL,
                    format!("sulion-dora:{repo}:{key}").as_bytes(),
                );
                sqlx::query(
                    "INSERT INTO dora_deployments(id, repo_name, source_key, revision, deployed_at, source_url) \
                     VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (repo_name, source_key) DO NOTHING",
                ).bind(id).bind(repo).bind(&key).bind(&run.head_sha).bind(at).bind(&run.html_url).execute(pool).await?;
            }
            if attempt < run.run_attempt || run.status == "completed" {
                sqlx::query("INSERT INTO dora_run_checks(repo_name, source_key, production_source) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
                    .bind(repo).bind(&key).bind(supported).execute(pool).await?;
            }
        }
    }
    let previous_coverage: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT dora_scanned_since FROM repo_runtime_state WHERE repo_name = $1",
    )
    .bind(repo)
    .fetch_one(pool)
    .await?;
    anyhow::ensure!(
        recognized || previous_coverage.is_some(),
        "No supported main-branch production deployment steps found."
    );
    let warning = collect_commits(pool, repo, path)
        .await
        .err()
        .map(|err| format!("Change lead time incomplete: {err:#}"));
    sqlx::query(
        "UPDATE repo_runtime_state SET dora_checked_at = $2, dora_scanned_since = $3 WHERE repo_name = $1",
    ).bind(repo).bind(now).bind(since).execute(pool).await?;
    Ok(warning)
}

async fn git_output(path: &Path, args: &[&str]) -> anyhow::Result<String> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-c")
        .arg(format!("safe.directory={}", path.display()))
        .arg("-C")
        .arg(path)
        .args(args)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output()).await??;
    anyhow::ensure!(
        output.status.success(),
        "Local Git history is unavailable for deployed revisions."
    );
    Ok(String::from_utf8(output.stdout)?)
}

async fn collect_commits(pool: &Pool, repo: &str, path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        git_output(path, &["rev-parse", "--is-shallow-repository"])
            .await?
            .trim()
            == "false",
        "Complete local Git history is required to measure change lead time."
    );
    let deployments: Vec<(Uuid, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, revision, deployed_at FROM dora_deployments WHERE repo_name = $1 AND NOT commits_collected ORDER BY deployed_at, id",
    ).bind(repo).fetch_all(pool).await?;
    for (id, revision, at) in deployments {
        let previous: Option<String> = sqlx::query_scalar(
            "SELECT revision FROM dora_deployments WHERE repo_name = $1 AND deployed_at < $2 \
             AND commits_collected ORDER BY deployed_at DESC, id DESC LIMIT 1",
        )
        .bind(repo)
        .bind(at)
        .fetch_optional(pool)
        .await?;
        let range = previous
            .as_ref()
            .map(|previous| format!("{previous}..{revision}"))
            .unwrap_or(revision);
        let output = git_output(path, &["log", "--format=%H %cI", &range, "--"]).await?;
        let mut tx = pool.begin().await?;
        let mut commits = Vec::new();
        for line in output.lines() {
            let (sha, date) = line
                .split_once(' ')
                .context("Invalid Git commit evidence")?;
            let committed = DateTime::parse_from_rfc3339(date)?.with_timezone(&Utc);
            commits.push((sha, committed));
        }
        for chunk in commits.chunks(1000) {
            let mut query = sqlx::QueryBuilder::new(
                "INSERT INTO dora_commit_deployments(repo_name, revision, deployment_id, committed_at, baseline) ",
            );
            query.push_values(chunk, |mut row, (sha, committed)| {
                row.push_bind(repo)
                    .push_bind(*sha)
                    .push_bind(id)
                    .push_bind(*committed)
                    .push_bind(previous.is_none());
            });
            query.push(" ON CONFLICT (repo_name, revision) DO UPDATE SET \
                deployment_id = EXCLUDED.deployment_id, committed_at = EXCLUDED.committed_at, baseline = EXCLUDED.baseline \
                WHERE (SELECT deployed_at FROM dora_deployments WHERE id = dora_commit_deployments.deployment_id) > ");
            query.push_bind(at).build().execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE dora_deployments SET commits_collected = TRUE WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(name: &str, conclusion: &str) -> Step {
        Step {
            name: name.into(),
            conclusion: Some(conclusion.into()),
            completed_at: Some(Utc::now()),
        }
    }

    #[test]
    fn deployment_evidence_uses_steps_and_survives_later_report_failure() {
        let jobs = [Job {
            steps: vec![
                step("Deploy to Komodo", "success"),
                step("Upload report", "failure"),
            ],
        }];
        assert!(deployed_at(&jobs).is_some());
        let jobs = [Job {
            steps: vec![
                step("Terraform deploy", "success"),
                step("Deploy to Komodo", "failure"),
            ],
        }];
        assert!(deployed_at(&jobs).is_none());
        let jobs = [Job {
            steps: vec![
                step("Test rust", "success"),
                step("Deploy to Komodo", "skipped"),
            ],
        }];
        assert!(deployed_at(&jobs).is_none());
        let jobs = [Job {
            steps: vec![step(
                "Run chris-arsenault/ahara/.github/actions/deploy-truenas@main",
                "success",
            )],
        }];
        assert!(deployed_at(&jobs).is_some());
    }
}
