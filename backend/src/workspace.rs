//! Filesystem surface for the repo workspace. Directory listings for
//! the left-nav tree, file reads for the preview tab, and upload
//! handling for drag-drop + paste-as-file.
//!
//! Path safety: callers pass repo-relative paths. We canonicalize
//! against the repo root and reject anything escaping it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::DiffStat;

#[derive(Debug, Serialize, Deserialize)]
pub struct DirEntryView {
    pub name: String,
    pub kind: String, // "file" | "dir"
    pub size: u64,
    pub mtime: Option<String>, // ISO 8601
    pub dirty: Option<String>, // 2-char status code, if any
    pub diff: Option<DiffStat>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DirListing {
    pub path: String, // canonicalized repo-relative path
    pub entries: Vec<DirEntryView>,
}

/// Resolve a repo-relative path against the repo root, rejecting any
/// attempt to escape the root. Returns the canonical absolute path +
/// the (possibly-normalized) relative path.
pub fn resolve_in_repo(repo_root: &Path, rel: &str) -> anyhow::Result<(PathBuf, String)> {
    if rel.starts_with('/') {
        anyhow::bail!("absolute path rejected");
    }
    // Reject obvious escapes without needing canonicalize (which fails
    // for paths that don't exist yet — matters for upload).
    for component in Path::new(rel).components() {
        use std::path::Component;
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                anyhow::bail!("path escape (..) rejected")
            }
            Component::Prefix(_) | Component::RootDir => {
                anyhow::bail!("absolute path rejected")
            }
        }
    }
    // `join("")` appends a separator, so the repo root itself would come back
    // as `/repo/` and compare unequal to every other spelling of the same
    // directory. An empty or `.` relative path means the root.
    let target = if rel.is_empty() || rel == "." {
        repo_root.to_path_buf()
    } else {
        repo_root.join(rel)
    };
    // If the path exists, canonicalize both sides and verify containment.
    if target.exists() {
        let root_real = repo_root.canonicalize()?;
        let target_real = target.canonicalize()?;
        if !target_real.starts_with(&root_real) {
            anyhow::bail!("path escape after canonicalize");
        }
    }
    Ok((target, rel.to_string()))
}

/// List a directory under the repo root. When `only_tracked` is true,
/// the listing is intersected with `git ls-files -co --exclude-standard`
/// so ignored paths don't show up.
pub async fn list_dir(
    repo_root: PathBuf,
    rel: String,
    only_tracked: bool,
    dirty_by_path: HashMap<String, String>,
    diff_stats_by_path: HashMap<String, DiffStat>,
) -> anyhow::Result<DirListing> {
    let (abs, rel_canon) = resolve_in_repo(&repo_root, &rel)?;
    if !abs.exists() {
        anyhow::bail!("not found: {}", abs.display());
    }
    if !abs.is_dir() {
        anyhow::bail!("not a directory: {}", abs.display());
    }

    let visible: Option<std::collections::HashSet<String>> = if only_tracked {
        Some(list_visible_paths(&repo_root, &rel_canon).await?)
    } else {
        None
    };

    let mut entries = Vec::new();
    let mut read = tokio::fs::read_dir(&abs).await?;
    while let Some(entry) = read.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            // Always hide the git metadata dir from the tree.
            continue;
        }
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let kind = if meta.is_dir() { "dir" } else { "file" };
        let rel_entry = if rel_canon.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", rel_canon, name)
        };

        // Filter against the visible set. Dirs always show if they
        // contain at least one visible path; files show iff in the set.
        if let Some(visible) = &visible {
            let ok = match kind {
                "file" => visible.contains(&rel_entry),
                "dir" => visible
                    .iter()
                    .any(|p| p.starts_with(&format!("{}/", rel_entry))),
                _ => false,
            };
            if !ok {
                continue;
            }
        }

        let mtime = meta
            .modified()
            .ok()
            .and_then(|m| chrono::DateTime::<chrono::Utc>::from(m).to_rfc3339().into());

        let dirty = dirty_by_path.get(&rel_entry).cloned().or_else(|| {
            // Directories carry a composite flag when any descendant is
            // dirty — "?" if any untracked, "M" otherwise.
            if kind == "dir" {
                let prefix = format!("{}/", rel_entry);
                let mut any = false;
                let mut only_untracked = true;
                for (p, code) in dirty_by_path.iter() {
                    if p.starts_with(&prefix) {
                        any = true;
                        if code != "??" {
                            only_untracked = false;
                        }
                    }
                }
                if any {
                    Some(if only_untracked { "??" } else { " M" }.to_string())
                } else {
                    None
                }
            } else {
                None
            }
        });
        let diff = diff_stats_by_path.get(&rel_entry).cloned().or_else(|| {
            if kind == "dir" {
                let prefix = format!("{rel_entry}/");
                let mut combined = DiffStat::default();
                let mut any = false;
                for (path, stat) in &diff_stats_by_path {
                    if !path.starts_with(&prefix) {
                        continue;
                    }
                    any = true;
                    combined.additions += stat.additions;
                    combined.deletions += stat.deletions;
                }
                if any {
                    Some(combined)
                } else {
                    None
                }
            } else {
                None
            }
        });

        entries.push(DirEntryView {
            name,
            kind: kind.to_string(),
            size: meta.len(),
            mtime,
            dirty,
            diff,
        });
    }

    // Sort: directories first, then files, alphabetical within groups.
    entries.sort_by(|a, b| match (a.kind.as_str(), b.kind.as_str()) {
        ("dir", "file") => std::cmp::Ordering::Less,
        ("file", "dir") => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(DirListing {
        path: rel_canon,
        entries,
    })
}

async fn list_visible_paths(
    repo_root: &Path,
    rel: &str,
) -> anyhow::Result<std::collections::HashSet<String>> {
    let repo_root = repo_root.to_path_buf();
    let subdir = rel.to_string();
    tokio::task::spawn_blocking(move || {
        let mut args: Vec<String> = vec![
            "-C".to_string(),
            repo_root.to_string_lossy().into_owned(),
            "ls-files".to_string(),
            "-co".to_string(),
            "--exclude-standard".to_string(),
        ];
        if !subdir.is_empty() {
            args.push("--".to_string());
            args.push(format!("{}/", subdir));
        }
        let out = std::process::Command::new("git").args(&args).output()?;
        if !out.status.success() {
            // `git ls-files` fails in a non-repo; fall back to showing
            // everything so the UI doesn't hang.
            return Ok(std::collections::HashSet::new());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text
            .lines()
            .map(|s| s.to_string())
            .collect::<std::collections::HashSet<_>>())
    })
    .await?
}

/// Install bytes atomically through directory descriptors, never through a
/// caller-controlled symlink. Final symlinks are replaced, not followed.
pub async fn write_file(
    repo_root: PathBuf,
    rel: String,
    bytes: Vec<u8>,
) -> anyhow::Result<PathBuf> {
    use crate::uploads::install::Directory;
    use tokio::io::AsyncWriteExt;
    if bytes.len() as i64 > crate::uploads::MAX_BYTES {
        anyhow::bail!("file exceeds upload size limit");
    }
    let (parent, filename) = rel.rsplit_once('/').unwrap_or(("", &rel));
    if !crate::uploads::model::valid_component(filename) || rel.starts_with('/') {
        anyhow::bail!("invalid upload path");
    }
    let directory = Directory::root(&repo_root)?.descend(parent, true)?;
    let mut temporary = directory.temporary()?;
    let mut file = tokio::fs::File::from_std(temporary.take_file());
    file.write_all(&bytes).await?;
    file.sync_all().await?;
    temporary.install_at(&repo_root, parent, filename)?;
    Ok(repo_root.join(rel))
}

/// Whether a repo name is safe to use as a single path component.
///
/// One rule, because there were three and they disagreed: the node rejected
/// `\` and `..` while the API and worktree layers accepted them, so whether a
/// name was valid depended on which layer looked at it first. This is the
/// strictest of the three.
pub fn is_valid_repo_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
}

pub fn looks_binary(bytes: &[u8]) -> bool {
    // Classic heuristic: NUL byte in the first 8KB = binary.
    bytes.iter().take(8192).any(|&b| b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn upload_cannot_escape_via_missing_parent_or_dangling_symlink() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        for path in ["escape/new/file", "../file", "/file"] {
            assert!(write_file(root.path().into(), path.into(), b"new".to_vec())
                .await
                .is_err());
        }
        assert!(!outside.path().join("new").exists());
        std::os::unix::fs::symlink(outside.path().join("absent"), root.path().join("file"))
            .unwrap();
        write_file(root.path().into(), "file".into(), b"new".to_vec())
            .await
            .unwrap();
        assert!(!outside.path().join("absent").exists());
        assert_eq!(std::fs::read(root.path().join("file")).unwrap(), b"new");
    }

    #[test]
    fn resolve_rejects_parent_traversal() {
        let tmp = tempdir().unwrap();
        assert!(resolve_in_repo(tmp.path(), "../etc/passwd").is_err());
        assert!(resolve_in_repo(tmp.path(), "a/../../b").is_err());
    }

    #[test]
    fn resolve_rejects_absolute() {
        let tmp = tempdir().unwrap();
        assert!(resolve_in_repo(tmp.path(), "/etc/passwd").is_err());
    }

    #[test]
    fn resolve_accepts_nested_rel() {
        let tmp = tempdir().unwrap();
        let (_abs, rel) = resolve_in_repo(tmp.path(), "src/lib.rs").unwrap();
        assert_eq!(rel, "src/lib.rs");
    }

    #[test]
    fn looks_binary_detects_nul_byte() {
        assert!(!super::looks_binary(b"hello world"));
        assert!(super::looks_binary(b"abc\x00def"));
    }
}
