//! GitHub Actions status for repositories with a GitHub origin.
//!
//! The node authenticates with the `GH_TOKEN` that the secret broker's
//! all-terminals grants give the `gh` program, redeemed through the same path
//! as `with-cred -- gh`. With it, every repository is checked every ten minutes and
//! private repositories the token can read are included. Without it the node
//! polls anonymously and shares GitHub's unauthenticated budget of 60 requests
//! per hour: active repositories are checked every ten minutes and idle ones
//! every six hours. Repositories that answer 404 are rechecked daily, and a
//! rate-limit response pauses all polling until GitHub's reset time.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::header::{HeaderMap, ACCEPT};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::db::Pool;

pub const GITHUB_API_BASE: &str = "https://api.github.com";
const GITHUB_WEB_BASE: &str = "https://github.com/";
const ACTIVE_CADENCE_SECS: i64 = 600;
const IDLE_CADENCE_SECS: i64 = 6 * 3600;
const UNAVAILABLE_CADENCE_SECS: i64 = 24 * 3600;
const ERROR_CADENCE_SECS: i64 = 1800;
const ACTIVE_WINDOW_HOURS: i64 = 24;
const DEFAULT_RATE_LIMIT_PAUSE_SECS: i64 = 3600;
/// How long a token read from the broker is reused before it is read again,
/// so a rotated or revoked token takes effect within one active cycle.
const TOKEN_TTL: Duration = Duration::from_secs(600);

/// Head or branch changes pull the next check to at most this far out, so a
/// push is picked up on the active cadence even from an idle repo.
pub const CHANGED_HEAD_CADENCE_SECS: i32 = ACTIVE_CADENCE_SECS as i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiState {
    Failed,
    InProgress,
    Succeeded,
}

impl CiState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::InProgress => "in_progress",
            Self::Succeeded => "succeeded",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "failed" => Some(Self::Failed),
            "in_progress" => Some(Self::InProgress),
            "succeeded" => Some(Self::Succeeded),
            _ => None,
        }
    }

    fn from_run(status: &str, conclusion: Option<&str>) -> Self {
        if status != "completed" {
            return Self::InProgress;
        }
        match conclusion {
            Some("success" | "neutral" | "skipped") => Self::Succeeded,
            _ => Self::Failed,
        }
    }
}

/// Latest Actions run on the repository's checked-out branch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoCiStatus {
    pub state: CiState,
    pub updated_at: DateTime<Utc>,
    pub run_url: String,
}

/// Stored CI columns as the API reads them. A run recorded for another
/// branch is hidden until the next check replaces it.
pub struct StoredCi<'a> {
    pub state: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub run_url: Option<&'a str>,
    pub run_updated_at: Option<DateTime<Utc>>,
}

impl StoredCi<'_> {
    pub fn view(&self, current_branch: Option<&str>) -> Option<RepoCiStatus> {
        if self.branch.is_none() || self.branch != current_branch {
            return None;
        }
        Some(RepoCiStatus {
            state: CiState::parse(self.state?)?,
            updated_at: self.run_updated_at?,
            run_url: self.run_url?.to_string(),
        })
    }
}

/// `(owner, name)` for a GitHub remote in scp, ssh, or http(s) form.
pub fn github_repo(origin_url: &str) -> Option<(String, String)> {
    let url = origin_url.trim();
    let path = if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else {
        let (_, rest) = url.split_once("://")?;
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?.split(':').next()?;
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        path
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some((owner.to_string(), name.to_string()))
}

pub fn github_web_url(origin_url: &str) -> Option<String> {
    github_repo(origin_url).map(|(owner, name)| format!("{GITHUB_WEB_BASE}{owner}/{name}"))
}

enum RunLookup {
    Run(RepoCiStatus),
    NoRuns,
    Unavailable,
    RateLimited(DateTime<Utc>),
}

#[derive(Deserialize)]
struct RunsResponse {
    workflow_runs: Vec<WorkflowRun>,
}

#[derive(Deserialize)]
struct WorkflowRun {
    status: String,
    conclusion: Option<String>,
    html_url: String,
    updated_at: DateTime<Utc>,
}

/// The broker credential id the CI poller registers on this host.
fn service_id() -> uuid::Uuid {
    let host = std::env::var("HOSTNAME").unwrap_or_default();
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("sulion-ci:{host}").as_bytes(),
    )
}

/// Where the node's GitHub token comes from.
pub enum GithubAuth {
    /// `GH_TOKEN` from the broker grants that apply to `gh`; anonymous when none.
    Broker,
    Static(String),
    Anonymous,
}

pub struct GithubCi {
    http: reqwest::Client,
    api_base: String,
    auth: GithubAuth,
    cached_token: tokio::sync::Mutex<Option<(Option<String>, Instant)>>,
    paused_until: Mutex<Option<DateTime<Utc>>>,
}

impl GithubCi {
    pub fn brokered() -> Self {
        Self::new(GITHUB_API_BASE, GithubAuth::Broker)
    }

    pub fn new(api_base: impl Into<String>, auth: GithubAuth) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("sulion/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(15))
            .build()
            .expect("build GitHub HTTP client");
        Self {
            http,
            api_base: api_base.into().trim_end_matches('/').to_string(),
            auth,
            cached_token: tokio::sync::Mutex::new(None),
            paused_until: Mutex::new(None),
        }
    }

    async fn token(&self) -> Option<String> {
        match &self.auth {
            GithubAuth::Anonymous => None,
            GithubAuth::Static(token) => Some(token.clone()),
            GithubAuth::Broker => {
                let mut cached = self.cached_token.lock().await;
                if let Some((token, read_at)) = cached.as_ref() {
                    if read_at.elapsed() < TOKEN_TTL {
                        return token.clone();
                    }
                }
                let token = crate::secret_pty::redeem_for_service(service_id(), "gh")
                    .await
                    .map(|env| env.and_then(|mut env| env.remove("GH_TOKEN")))
                    .unwrap_or_else(|err| {
                        tracing::warn!(%err, "redeem GitHub token from the secret broker; polling anonymously");
                        None
                    });
                *cached = Some((token.clone(), Instant::now()));
                token
            }
        }
    }

    /// Drop a token GitHub rejected so the next check reads it again.
    async fn forget_token(&self) {
        *self.cached_token.lock().await = None;
    }

    fn paused_until(&self) -> Option<DateTime<Utc>> {
        let paused = *self.paused_until.lock().expect("GitHub pause lock");
        paused.filter(|until| *until > Utc::now())
    }

    fn pause(&self, until: DateTime<Utc>) {
        *self.paused_until.lock().expect("GitHub pause lock") = Some(until);
    }

    async fn latest_run(
        &self,
        owner: &str,
        name: &str,
        branch: &str,
        token: Option<&str>,
    ) -> anyhow::Result<RunLookup> {
        let mut request = self
            .http
            .get(format!(
                "{}/repos/{owner}/{name}/actions/runs",
                self.api_base
            ))
            .query(&[("branch", branch), ("per_page", "1")])
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .context("request GitHub Actions runs")?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED && token.is_some() {
            self.forget_token().await;
            anyhow::bail!("GitHub rejected the broker's GH_TOKEN");
        }
        if status == StatusCode::NOT_FOUND {
            return Ok(RunLookup::Unavailable);
        }
        if let Some(until) = rate_limit_reset(status, response.headers()) {
            return Ok(RunLookup::RateLimited(until));
        }
        if !status.is_success() {
            anyhow::bail!("GitHub Actions runs returned {status}");
        }
        let body: RunsResponse = response
            .json()
            .await
            .context("decode GitHub Actions runs")?;
        let Some(run) = body.workflow_runs.into_iter().next() else {
            return Ok(RunLookup::NoRuns);
        };
        if !run.html_url.starts_with(GITHUB_WEB_BASE) {
            anyhow::bail!("GitHub run URL is outside github.com");
        }
        Ok(RunLookup::Run(RepoCiStatus {
            state: CiState::from_run(&run.status, run.conclusion.as_deref()),
            updated_at: run.updated_at,
            run_url: run.html_url,
        }))
    }
}

fn rate_limit_reset(status: StatusCode, headers: &HeaderMap) -> Option<DateTime<Utc>> {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let exhausted = header("x-ratelimit-remaining") == Some("0");
    let retry_after = header("retry-after").and_then(|value| value.parse::<i64>().ok());
    if status != StatusCode::TOO_MANY_REQUESTS
        && !(status == StatusCode::FORBIDDEN && (exhausted || retry_after.is_some()))
    {
        return None;
    }
    if let Some(secs) = retry_after {
        return Some(Utc::now() + chrono::Duration::seconds(secs));
    }
    header("x-ratelimit-reset")
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|epoch| Utc.timestamp_opt(epoch, 0).single())
        .or_else(|| Some(Utc::now() + chrono::Duration::seconds(DEFAULT_RATE_LIMIT_PAUSE_SECS)))
}

fn cadence_secs(
    authenticated: bool,
    head_committed_at: Option<DateTime<Utc>>,
    state: Option<CiState>,
) -> i64 {
    let recent_commit = head_committed_at
        .is_some_and(|at| Utc::now() - at < chrono::Duration::hours(ACTIVE_WINDOW_HOURS));
    if authenticated || recent_commit || state == Some(CiState::InProgress) {
        ACTIVE_CADENCE_SECS
    } else {
        IDLE_CADENCE_SECS
    }
}

type CiTargetRow = (Option<String>, Option<String>, Option<DateTime<Utc>>);

/// Check one repository's latest run and schedule its next check. Failures
/// keep the previous run visible and retry on the error cadence.
pub async fn reconcile_repo_ci(pool: &Pool, ci: &GithubCi, repo: &str) -> anyhow::Result<()> {
    let row: Option<CiTargetRow> = sqlx::query_as(
        "SELECT origin_url, branch, head_committed_at FROM repo_runtime_state WHERE repo_name = $1",
    )
    .bind(repo)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("load CI target for {repo}"))?;
    let Some((origin_url, branch, head_committed_at)) = row else {
        return Ok(());
    };
    let target = origin_url.as_deref().and_then(github_repo).zip(branch);
    let Some(((owner, name), branch)) = target else {
        return store_run(pool, repo, None, None, UNAVAILABLE_CADENCE_SECS).await;
    };
    if let Some(until) = ci.paused_until() {
        return schedule_at(pool, repo, until).await;
    }
    let token = ci.token().await;
    let authenticated = token.is_some();
    match ci
        .latest_run(&owner, &name, &branch, token.as_deref())
        .await
    {
        Ok(RunLookup::Run(run)) => {
            let cadence = cadence_secs(authenticated, head_committed_at, Some(run.state));
            store_run(pool, repo, Some(&branch), Some(&run), cadence).await
        }
        Ok(RunLookup::NoRuns) => {
            let cadence = cadence_secs(authenticated, head_committed_at, None);
            store_run(pool, repo, Some(&branch), None, cadence).await
        }
        Ok(RunLookup::Unavailable) => {
            store_run(pool, repo, Some(&branch), None, UNAVAILABLE_CADENCE_SECS).await
        }
        Ok(RunLookup::RateLimited(until)) => {
            tracing::info!(%until, "GitHub API rate limit reached; pausing CI checks");
            ci.pause(until);
            schedule_at(pool, repo, until).await
        }
        Err(err) => {
            sqlx::query(
                "UPDATE repo_runtime_state \
                    SET ci_error = $2, \
                        ci_checked_at = NOW(), \
                        next_ci_at = NOW() + make_interval(secs => $3) \
                  WHERE repo_name = $1",
            )
            .bind(repo)
            .bind(format!("{err:#}"))
            .bind(ERROR_CADENCE_SECS as i32)
            .execute(pool)
            .await
            .with_context(|| format!("record CI error for {repo}"))?;
            Err(err)
        }
    }
}

async fn store_run(
    pool: &Pool,
    repo: &str,
    branch: Option<&str>,
    run: Option<&RepoCiStatus>,
    cadence_secs: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE repo_runtime_state \
            SET ci_state = $2, \
                ci_branch = $3, \
                ci_run_url = $4, \
                ci_run_updated_at = $5, \
                ci_error = NULL, \
                ci_checked_at = NOW(), \
                next_ci_at = NOW() + make_interval(secs => $6) \
          WHERE repo_name = $1",
    )
    .bind(repo)
    .bind(run.map(|run| run.state.as_str()))
    .bind(branch)
    .bind(run.map(|run| run.run_url.as_str()))
    .bind(run.map(|run| run.updated_at))
    .bind(cadence_secs as i32)
    .execute(pool)
    .await
    .with_context(|| format!("store CI status for {repo}"))?;
    Ok(())
}

async fn schedule_at(pool: &Pool, repo: &str, at: DateTime<Utc>) -> anyhow::Result<()> {
    sqlx::query("UPDATE repo_runtime_state SET next_ci_at = $2 WHERE repo_name = $1")
        .bind(repo)
        .bind(at)
        .execute(pool)
        .await
        .with_context(|| format!("reschedule CI check for {repo}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_remote_forms() {
        let expected = Some(("chris-arsenault".to_string(), "sulion".to_string()));
        for url in [
            "git@github.com:chris-arsenault/sulion.git",
            "git@github.com:chris-arsenault/sulion",
            "https://github.com/chris-arsenault/sulion.git",
            "https://github.com/chris-arsenault/sulion/",
            "ssh://git@github.com/chris-arsenault/sulion.git",
            "https://github.com:443/chris-arsenault/sulion",
        ] {
            assert_eq!(github_repo(url), expected, "{url}");
        }
        assert_eq!(
            github_web_url("git@github.com:excellaco/oig-cdo.git").as_deref(),
            Some("https://github.com/excellaco/oig-cdo")
        );
    }

    #[test]
    fn rejects_non_github_remotes() {
        for url in [
            "git@gitlab.com:owner/repo.git",
            "https://github.com.evil.example/owner/repo",
            "https://github.com/owner",
            "https://github.com/owner/repo/tree/main",
            "/srv/git/repo.git",
            "",
        ] {
            assert_eq!(github_repo(url), None, "{url}");
        }
    }

    #[test]
    fn maps_run_status_to_three_states() {
        assert_eq!(CiState::from_run("queued", None), CiState::InProgress);
        assert_eq!(CiState::from_run("in_progress", None), CiState::InProgress);
        assert_eq!(
            CiState::from_run("completed", Some("success")),
            CiState::Succeeded
        );
        assert_eq!(
            CiState::from_run("completed", Some("skipped")),
            CiState::Succeeded
        );
        assert_eq!(
            CiState::from_run("completed", Some("failure")),
            CiState::Failed
        );
        assert_eq!(
            CiState::from_run("completed", Some("cancelled")),
            CiState::Failed
        );
        assert_eq!(CiState::from_run("completed", None), CiState::Failed);
    }

    #[test]
    fn stored_run_is_hidden_after_branch_change() {
        let at = Utc::now();
        let stored = StoredCi {
            state: Some("succeeded"),
            branch: Some("main"),
            run_url: Some("https://github.com/o/r/actions/runs/1"),
            run_updated_at: Some(at),
        };
        assert_eq!(
            stored.view(Some("main")).map(|ci| ci.state),
            Some(CiState::Succeeded)
        );
        assert!(stored.view(Some("feature")).is_none());
        assert!(stored.view(None).is_none());
    }

    #[test]
    fn rate_limit_detection_uses_github_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        headers.insert("x-ratelimit-reset", "2000000000".parse().unwrap());
        assert_eq!(
            rate_limit_reset(StatusCode::FORBIDDEN, &headers),
            Utc.timestamp_opt(2_000_000_000, 0).single()
        );
        assert!(rate_limit_reset(StatusCode::FORBIDDEN, &HeaderMap::new()).is_none());
        assert!(rate_limit_reset(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new()).is_some());
    }

    #[test]
    fn anonymous_polling_uses_the_short_cadence_only_for_active_repos() {
        let recent = Some(Utc::now() - chrono::Duration::hours(1));
        let old = Some(Utc::now() - chrono::Duration::days(3));
        assert_eq!(cadence_secs(false, recent, None), ACTIVE_CADENCE_SECS);
        assert_eq!(
            cadence_secs(false, old, Some(CiState::InProgress)),
            ACTIVE_CADENCE_SECS
        );
        assert_eq!(
            cadence_secs(false, old, Some(CiState::Succeeded)),
            IDLE_CADENCE_SECS
        );
        assert_eq!(cadence_secs(false, None, None), IDLE_CADENCE_SECS);
    }

    #[test]
    fn authenticated_polling_checks_every_repo_on_the_short_cadence() {
        let old = Some(Utc::now() - chrono::Duration::days(3));
        assert_eq!(
            cadence_secs(true, old, Some(CiState::Succeeded)),
            ACTIVE_CADENCE_SECS
        );
        assert_eq!(cadence_secs(true, None, None), ACTIVE_CADENCE_SECS);
    }
}
