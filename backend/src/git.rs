//! Shell-out helpers for repo git state. All commands run under
//! `tokio::task::spawn_blocking` because `git status` on TrueNAS-
//! backed bind mounts can take tens of ms and we don't want to block
//! the tokio runtime worker while waiting on disk.
//!
//! Output parsing sticks to porcelain v1 (`-z`) because it's stable
//! and tiny; v2 adds detail we don't render.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub committed_at: String, // ISO 8601
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct GitStatus {
    pub branch: Option<String>,
    pub uncommitted_count: usize,
    pub untracked_count: usize,
    pub last_commit: Option<Commit>,
    pub recent_commits: Vec<Commit>,
    /// Map repo-relative path -> 2-char status code (e.g. " M", "??",
    /// "A ", "R "). The first char is index state, second is worktree.
    pub dirty_by_path: HashMap<String, String>,
    /// Map repo-relative path -> current working-copy churn relative to HEAD.
    pub diff_stats_by_path: HashMap<String, DiffStat>,
}

#[derive(Debug, Serialize, Clone, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub additions: usize,
    pub deletions: usize,
}

/// Is this path a git repo? Cheap filesystem check; no subprocess.
pub fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists()
}

pub async fn read_status(repo_path: PathBuf) -> anyhow::Result<GitStatus> {
    // Serialize the subprocesses; status + log + branch on the same repo
    // share the index lock and fight if parallel.
    tokio::task::spawn_blocking(move || read_status_blocking(&repo_path)).await?
}

fn read_status_blocking(repo_path: &Path) -> anyhow::Result<GitStatus> {
    let mut status = GitStatus::default();

    if !is_git_repo(repo_path) {
        return Ok(status);
    }

    status.branch = current_branch(repo_path)?;

    let porcelain = run_git(repo_path, &["status", "--porcelain=v1", "-z"])?;
    parse_porcelain(&porcelain, &mut status);
    status.diff_stats_by_path = read_diff_stats(repo_path, &status.dirty_by_path)?;

    if let Some((last, recent)) = read_log(repo_path, 4)? {
        status.last_commit = Some(last);
        status.recent_commits = recent;
    }
    Ok(status)
}

fn current_branch(repo_path: &Path) -> anyhow::Result<Option<String>> {
    let out = run_git(repo_path, &["branch", "--show-current"])?;
    let s = String::from_utf8_lossy(&out).trim().to_string();
    Ok(if s.is_empty() { None } else { Some(s) })
}

fn run_git(repo_path: &Path, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .output()?;
    if !out.status.success() {
        // Non-fatal: repo may be in a degraded state. Return empty and
        // let callers degrade gracefully rather than 500.
        tracing::debug!(
            repo = %repo_path.display(),
            args = ?args,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "git command non-zero"
        );
        return Ok(Vec::new());
    }
    Ok(out.stdout)
}

/// Parse `git status --porcelain=v1 -z` output. With `-z`, records are
/// NUL-terminated and status codes are never renamed across NULs except
/// for the R/C (rename/copy) case which emits two paths: `R<sp>new\0orig\0`.
fn parse_porcelain(bytes: &[u8], status: &mut GitStatus) {
    // Split on NUL, but the rename/copy case eats the following record.
    let mut iter = bytes.split(|&b| b == 0).peekable();
    while let Some(record) = iter.next() {
        if record.is_empty() {
            // Trailing NUL from the last real record.
            continue;
        }
        if record.len() < 3 {
            continue;
        }
        // Layout: XY<space>PATH  (3-byte prefix + path)
        let code = std::str::from_utf8(&record[..2])
            .unwrap_or("  ")
            .to_string();
        let path = String::from_utf8_lossy(&record[3..]).into_owned();

        match code.as_str() {
            "R " | "RM" | "C " | "CM" => {
                // Next record is the original path; consume and discard.
                let _ = iter.next();
            }
            _ => {}
        }

        status.uncommitted_count += 1;
        if code == "??" {
            status.untracked_count += 1;
        }
        status.dirty_by_path.insert(path, code);
    }
}

/// Returns (latest, up-to-3-most-recent) commits, if any.
fn read_log(repo_path: &Path, want: usize) -> anyhow::Result<Option<(Commit, Vec<Commit>)>> {
    // 0x1e (record separator) between commits; 0x1f (unit separator)
    // between fields. Picks characters that never appear in real subject
    // lines.
    let fmt = format!("-{}", want);
    let out = run_git(
        repo_path,
        &["log", &fmt, "--no-color", "--format=%h%x1f%s%x1f%cI%x1e"],
    )?;
    let text = String::from_utf8_lossy(&out);
    let mut commits: Vec<Commit> = Vec::new();
    for record in text.split('\x1e') {
        let record = record.trim_matches('\n');
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(3, '\x1f');
        let sha = parts.next().unwrap_or("").to_string();
        let subject = parts.next().unwrap_or("").to_string();
        let committed_at = parts.next().unwrap_or("").to_string();
        if sha.is_empty() {
            continue;
        }
        commits.push(Commit {
            sha,
            subject,
            committed_at,
        });
    }
    if commits.is_empty() {
        return Ok(None);
    }
    let last = commits[0].clone();
    let recent = commits.iter().take(3).cloned().collect();
    Ok(Some((last, recent)))
}

fn read_diff_stats(
    repo_path: &Path,
    dirty_by_path: &HashMap<String, String>,
) -> anyhow::Result<HashMap<String, DiffStat>> {
    let out = run_git(repo_path, &["diff", "HEAD", "--numstat"])?;
    let text = String::from_utf8_lossy(&out);
    let mut stats = HashMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let additions = parts.next().unwrap_or("");
        let deletions = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("").trim();
        if path.is_empty() {
            continue;
        }
        let Some(path) = path.rsplit('\t').next() else {
            continue;
        };
        stats.insert(
            path.to_string(),
            DiffStat {
                additions: additions.parse::<usize>().unwrap_or(0),
                deletions: deletions.parse::<usize>().unwrap_or(0),
            },
        );
    }

    for (path, code) in dirty_by_path {
        if code != "??" || stats.contains_key(path) {
            continue;
        }
        let additions = std::fs::read_to_string(repo_path.join(path))
            .map(|body| body.lines().count())
            .unwrap_or(0);
        stats.insert(
            path.clone(),
            DiffStat {
                additions,
                deletions: 0,
            },
        );
    }

    Ok(stats)
}

/// Read unified diff for a repo (whole, or a single path). Returns the
/// raw diff bytes. Includes untracked files by using `--no-index` as a
/// fallback when `--cached` doesn't surface them.
pub async fn read_diff(repo_path: PathBuf, path: Option<String>) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || read_diff_blocking(&repo_path, path.as_deref())).await?
}

fn read_diff_blocking(repo_path: &Path, path: Option<&str>) -> anyhow::Result<String> {
    if !is_git_repo(repo_path) {
        return Ok(String::new());
    }
    let mut args = vec!["diff", "HEAD", "--no-color"];
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    let out = run_git(repo_path, &args)?;
    let mut diff = String::from_utf8_lossy(&out).into_owned();

    // Untracked files don't show up in `git diff HEAD`. Append a
    // synthetic diff header + content so the UI sees them. Keep this
    // simple — single-file only, no binary detection; the frontend
    // caps render size.
    if path.is_none() {
        let status = read_status_blocking(repo_path)?;
        for (p, code) in status.dirty_by_path.iter() {
            if code == "??" {
                let full = repo_path.join(p);
                if let Ok(contents) = std::fs::read_to_string(&full) {
                    diff.push_str(&format!(
                        "diff --git a/{p} b/{p}\nnew file mode 100644\n--- /dev/null\n+++ b/{p}\n"
                    ));
                    for line in contents.lines() {
                        diff.push('+');
                        diff.push_str(line);
                        diff.push('\n');
                    }
                }
            }
        }
    } else if path.is_some() && diff.is_empty() {
        // Probably untracked. Dump content as a new-file diff.
        if let Some(p) = path {
            let full = repo_path.join(p);
            if let Ok(contents) = std::fs::read_to_string(&full) {
                diff.push_str(&format!(
                    "diff --git a/{p} b/{p}\nnew file mode 100644\n--- /dev/null\n+++ b/{p}\n"
                ));
                for line in contents.lines() {
                    diff.push('+');
                    diff.push_str(line);
                    diff.push('\n');
                }
            }
        }
    }
    Ok(diff)
}

/// Stage (git add) or unstage (git reset) a specific path.
pub async fn stage_path(repo_path: PathBuf, rel: String, stage: bool) -> anyhow::Result<()> {
    tokio::task::spawn_blocking(move || {
        let args: &[&str] = if stage {
            &["add", "--", rel.as_str()]
        } else {
            &["reset", "HEAD", "--", rel.as_str()]
        };
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo_path)
            .args(args)
            .output()?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            return Err(anyhow::anyhow!(
                "git {} {} failed: {}",
                if stage { "add" } else { "reset" },
                rel,
                stderr
            ));
        }
        Ok(())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_parse_basic() {
        // Two modified files + one untracked, -z terminated.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b" M src/lib.rs");
        buf.push(0);
        buf.extend_from_slice(b"?? new.txt");
        buf.push(0);
        buf.extend_from_slice(b"A  added.rs");
        buf.push(0);
        let mut status = GitStatus::default();
        parse_porcelain(&buf, &mut status);
        assert_eq!(status.uncommitted_count, 3);
        assert_eq!(status.untracked_count, 1);
        assert_eq!(status.dirty_by_path.get("src/lib.rs").unwrap(), " M");
        assert_eq!(status.dirty_by_path.get("new.txt").unwrap(), "??");
        assert_eq!(status.dirty_by_path.get("added.rs").unwrap(), "A ");
        assert!(status.diff_stats_by_path.is_empty());
    }

    #[test]
    fn porcelain_rename_eats_orig_path() {
        // R  newname.rs\0oldname.rs\0??  untracked.txt\0
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"R  newname.rs");
        buf.push(0);
        buf.extend_from_slice(b"oldname.rs");
        buf.push(0);
        buf.extend_from_slice(b"?? untracked.txt");
        buf.push(0);
        let mut status = GitStatus::default();
        parse_porcelain(&buf, &mut status);
        assert_eq!(status.uncommitted_count, 2);
        assert_eq!(status.dirty_by_path.get("newname.rs").unwrap(), "R ");
        // The orig path is NOT inserted.
        assert!(!status.dirty_by_path.contains_key("oldname.rs"));
        assert_eq!(status.dirty_by_path.get("untracked.txt").unwrap(), "??");
    }
}
