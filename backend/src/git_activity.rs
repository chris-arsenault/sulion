use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoGitActivity {
    pub repo: String,
    pub commits_24h: i64,
    pub commits_7d: i64,
    pub insertions_24h: i64,
    pub deletions_24h: i64,
    pub insertions_7d: i64,
    pub deletions_7d: i64,
    pub agent_commits_7d: i64,
    pub human_commits_7d: i64,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub daily: Vec<GitDay>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDay {
    pub day: NaiveDate,
    pub commits: i64,
    pub insertions: i64,
    pub deletions: i64,
}

struct CommitStat {
    epoch: i64,
    insertions: i64,
    deletions: i64,
    agent: bool,
}

pub fn empty_activity(name: &str) -> RepoGitActivity {
    RepoGitActivity {
        repo: name.to_string(),
        commits_24h: 0,
        commits_7d: 0,
        insertions_24h: 0,
        deletions_24h: 0,
        insertions_7d: 0,
        deletions_7d: 0,
        agent_commits_7d: 0,
        human_commits_7d: 0,
        last_commit_at: None,
        daily: Vec::new(),
    }
}

pub async fn scan_repo_git(name: &str, path: &Path) -> anyhow::Result<RepoGitActivity> {
    let meta = git_stdout(
        path,
        &[
            "log",
            "--since=14.days",
            "--pretty=%H%x1f%ct%x1f%(trailers:key=Co-authored-by,valueonly,separator=;)",
        ],
    )
    .await?;
    let mut commits: HashMap<String, CommitStat> = HashMap::new();
    for line in meta.lines() {
        let mut parts = line.split('\u{1f}');
        let (Some(hash), Some(epoch)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(epoch) = epoch.trim().parse::<i64>() else {
            continue;
        };
        let trailers = parts.next().unwrap_or("").to_ascii_lowercase();
        let agent = trailers.contains("claude") || trailers.contains("codex");
        commits.insert(
            hash.to_string(),
            CommitStat {
                epoch,
                insertions: 0,
                deletions: 0,
                agent,
            },
        );
    }

    let numstat = git_stdout(
        path,
        &["log", "--since=14.days", "--pretty=%x01%H", "--numstat"],
    )
    .await?;
    let mut current: Option<&mut CommitStat> = None;
    for line in numstat.lines() {
        if let Some(hash) = line.strip_prefix('\u{01}') {
            current = commits.get_mut(hash.trim());
            continue;
        }
        let Some(stat) = current.as_deref_mut() else {
            continue;
        };
        let mut cols = line.split('\t');
        let (Some(adds), Some(dels)) = (cols.next(), cols.next()) else {
            continue;
        };
        if let (Ok(adds), Ok(dels)) = (adds.trim().parse::<i64>(), dels.trim().parse::<i64>()) {
            stat.insertions += adds;
            stat.deletions += dels;
        }
    }

    let now = Utc::now().timestamp();
    let day_ago = now - 24 * 3600;
    let week_ago = now - 7 * 24 * 3600;
    let mut activity = empty_activity(name);
    let mut daily: HashMap<NaiveDate, GitDay> = HashMap::new();
    for stat in commits.values() {
        let at = Utc
            .timestamp_opt(stat.epoch, 0)
            .single()
            .unwrap_or_else(Utc::now);
        if activity.last_commit_at.is_none_or(|prev| at > prev) {
            activity.last_commit_at = Some(at);
        }
        if stat.epoch >= week_ago {
            activity.commits_7d += 1;
            activity.insertions_7d += stat.insertions;
            activity.deletions_7d += stat.deletions;
            if stat.agent {
                activity.agent_commits_7d += 1;
            } else {
                activity.human_commits_7d += 1;
            }
        }
        if stat.epoch >= day_ago {
            activity.commits_24h += 1;
            activity.insertions_24h += stat.insertions;
            activity.deletions_24h += stat.deletions;
        }
        let day = at.date_naive();
        let entry = daily.entry(day).or_insert(GitDay {
            day,
            commits: 0,
            insertions: 0,
            deletions: 0,
        });
        entry.commits += 1;
        entry.insertions += stat.insertions;
        entry.deletions += stat.deletions;
    }
    let mut days: Vec<GitDay> = daily.into_values().collect();
    days.sort_by_key(|day| day.day);
    activity.daily = days;
    Ok(activity)
}

async fn git_stdout(path: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("git")
        .arg("-c")
        .arg(format!("safe.directory={}", path.display()))
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
