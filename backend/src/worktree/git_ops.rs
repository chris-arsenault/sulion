use std::path::Path;

use anyhow::Context;

use super::WorkspaceRecord;
use crate::git::GitStatus;

pub(super) fn branch_component(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    if out.is_empty() {
        "repo".to_string()
    } else {
        out
    }
}

pub(super) async fn current_branch(repo_path: &Path) -> anyhow::Result<Option<String>> {
    let out = run_git_capture(repo_path, &["branch", "--show-current"]).await?;
    let branch = out.trim().to_string();
    Ok((!branch.is_empty()).then_some(branch))
}

pub(super) async fn rev_parse(repo_path: &Path, rev: &str) -> anyhow::Result<String> {
    let out = run_git_capture(repo_path, &["rev-parse", rev]).await?;
    let value = out.trim().to_string();
    if value.is_empty() {
        anyhow::bail!("git rev-parse {rev} returned empty output");
    }
    Ok(value)
}

pub(super) async fn git_branch_exists(repo_path: &Path, branch: &str) -> anyhow::Result<bool> {
    let repo_path = repo_path.to_path_buf();
    let branch = branch.to_string();
    tokio::task::spawn_blocking(move || {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo_path)
            .args(["show-ref", "--verify", "--quiet"])
            .arg(format!("refs/heads/{branch}"))
            .status()?;
        Ok(status.success())
    })
    .await?
}

pub(super) async fn unmerged_branch_commit_count(
    repo_path: &Path,
    workspace: &WorkspaceRecord,
    branch: &str,
) -> anyhow::Result<u64> {
    if let Some(target) = workspace
        .merge_target
        .as_deref()
        .or(workspace.base_ref.as_deref())
        .filter(|target| !target.is_empty())
    {
        match rev_list_count(repo_path, &format!("{target}..{branch}")).await {
            Ok(count) => return Ok(count),
            Err(err) => {
                tracing::debug!(
                    %branch,
                    %target,
                    %err,
                    "workspace merge target history check failed; falling back to base sha"
                );
            }
        }
    }

    if let Some(base_sha) = workspace.base_sha.as_deref() {
        return rev_list_count(repo_path, &format!("{base_sha}..{branch}")).await;
    }

    anyhow::bail!("workspace branch history cannot be checked; retry with force to delete it")
}

pub(super) async fn git_worktree_registered(
    repo_path: &Path,
    workspace_path: &Path,
) -> anyhow::Result<bool> {
    let out = run_git_capture(repo_path, &["worktree", "list", "--porcelain"]).await?;
    Ok(out.lines().any(|line| {
        line.strip_prefix("worktree ")
            .is_some_and(|path| Path::new(path) == workspace_path)
    }))
}

pub(super) async fn run_git_checked(repo_path: &Path, args: &[&str]) -> anyhow::Result<()> {
    run_git_capture(repo_path, args).await.map(|_| ())
}

pub(super) fn status_fingerprint(status: &GitStatus) -> String {
    let mut parts = Vec::new();
    parts.push(format!("branch={}", status.branch.as_deref().unwrap_or("")));
    parts.push(format!(
        "head={}",
        status
            .last_commit
            .as_ref()
            .map(|commit| commit.sha.as_str())
            .unwrap_or("")
    ));
    let mut dirty = status.dirty_by_path.iter().collect::<Vec<_>>();
    dirty.sort_by(|left, right| left.0.cmp(right.0));
    for (path, code) in dirty {
        let diff = status.diff_stats_by_path.get(path);
        let additions = diff.map(|d| d.additions).unwrap_or(0);
        let deletions = diff.map(|d| d.deletions).unwrap_or(0);
        parts.push(format!("{path}:{code}:{additions}:{deletions}"));
    }
    parts.join("\n")
}

async fn rev_list_count(repo_path: &Path, range: &str) -> anyhow::Result<u64> {
    let out = run_git_capture(repo_path, &["rev-list", "--count", range]).await?;
    out.trim()
        .parse::<u64>()
        .with_context(|| format!("parse git rev-list count for {range}"))
}

async fn run_git_capture(repo_path: &Path, args: &[&str]) -> anyhow::Result<String> {
    let repo_path = repo_path.to_path_buf();
    let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo_path)
            .args(&args)
            .output()?;
        if !out.status.success() {
            anyhow::bail!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_component_keeps_git_safe_chars() {
        assert_eq!(branch_component("the-canonry_game.1"), "the-canonry_game.1");
    }

    #[test]
    fn branch_component_replaces_unsafe_chars() {
        assert_eq!(branch_component("bad/repo name"), "bad-repo-name");
    }
}
