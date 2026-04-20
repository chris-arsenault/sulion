//! Global prompt/reference library stored under Sulion's own
//! app directory. These entries are intentionally lightweight:
//!
//! - prompts are reusable instructions the user injects into the
//!   active terminal
//! - references are saved assistant outputs the user wants to revisit
//!
//! Layout: `<library_root>/<kind>/<slug>.md`

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryKind {
    References,
    Prompts,
}

impl LibraryKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "references" | "refs" => Some(Self::References),
            "prompts" => Some(Self::Prompts),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::References => "references",
            Self::Prompts => "prompts",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    pub slug: String,
    pub name: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct SaveInput {
    pub name: String,
    pub body: String,
}

pub fn sanitise_slug(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(trimmed.len());
    for c in trimmed.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            out.push(c);
        } else if c.is_whitespace() {
            out.push('-');
        }
    }
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() || out.starts_with('.') {
        return None;
    }
    Some(out)
}

fn library_dir(library_root: &Path, kind: LibraryKind) -> PathBuf {
    library_root.join(kind.as_str())
}

fn entry_path(library_root: &Path, kind: LibraryKind, slug: &str) -> PathBuf {
    library_dir(library_root, kind).join(format!("{slug}.md"))
}

pub fn next_available_slug(library_root: &Path, kind: LibraryKind, desired: &str) -> String {
    if !entry_path(library_root, kind, desired).exists() {
        return desired.to_string();
    }

    let mut suffix = 2;
    loop {
        let candidate = format!("{desired}-{suffix}");
        if !entry_path(library_root, kind, &candidate).exists() {
            return candidate;
        }
        suffix += 1;
    }
}

pub async fn list(library_root: &Path, kind: LibraryKind) -> anyhow::Result<Vec<LibraryEntry>> {
    let dir = library_dir(library_root, kind);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    let mut rd = tokio::fs::read_dir(&dir).await?;
    while let Some(entry) = rd.next_entry().await? {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !file_name.ends_with(".md") {
            continue;
        }
        let slug = file_name.trim_end_matches(".md").to_string();
        let full = entry.path();
        match read_file(&full, &slug).await {
            Ok(entry) => entries.push(entry),
            Err(err) => tracing::warn!(%err, path = %full.display(), "library read failed"),
        }
    }

    entries.sort_by(|a, b| {
        b.updated_at
            .as_deref()
            .unwrap_or("")
            .cmp(a.updated_at.as_deref().unwrap_or(""))
            .then_with(|| {
                b.created_at
                    .as_deref()
                    .unwrap_or("")
                    .cmp(a.created_at.as_deref().unwrap_or(""))
            })
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

pub async fn read(
    library_root: &Path,
    kind: LibraryKind,
    slug: &str,
) -> anyhow::Result<Option<LibraryEntry>> {
    let slug = match sanitise_slug(slug) {
        Some(slug) => slug,
        None => return Ok(None),
    };
    let path = entry_path(library_root, kind, &slug);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(read_file(&path, &slug).await?))
}

pub async fn save(
    library_root: &Path,
    kind: LibraryKind,
    slug: &str,
    entry: SaveInput,
) -> anyhow::Result<LibraryEntry> {
    let slug = sanitise_slug(slug).ok_or_else(|| anyhow::anyhow!("invalid slug"))?;
    let dir = library_dir(library_root, kind);
    tokio::fs::create_dir_all(&dir).await?;
    let path = entry_path(library_root, kind, &slug);

    let existing = if path.exists() {
        read_file(&path, &slug).await.ok()
    } else {
        None
    };
    let now = chrono::Utc::now().to_rfc3339();
    let full_entry = LibraryEntry {
        slug: slug.clone(),
        name: entry.name.trim().to_string(),
        created_at: existing
            .as_ref()
            .and_then(|entry| entry.created_at.clone())
            .or(Some(now.clone())),
        updated_at: Some(now),
        body: entry.body,
    };

    tokio::fs::write(&path, render_entry(&full_entry)).await?;
    Ok(full_entry)
}

pub async fn delete(library_root: &Path, kind: LibraryKind, slug: &str) -> anyhow::Result<bool> {
    let slug = match sanitise_slug(slug) {
        Some(slug) => slug,
        None => return Ok(false),
    };
    let path = entry_path(library_root, kind, &slug);
    if !path.exists() {
        return Ok(false);
    }
    tokio::fs::remove_file(&path).await?;
    Ok(true)
}

async fn read_file(path: &Path, slug: &str) -> anyhow::Result<LibraryEntry> {
    let raw = tokio::fs::read_to_string(path).await?;
    Ok(parse_entry(&raw, slug))
}

fn parse_entry(raw: &str, slug: &str) -> LibraryEntry {
    let mut body_start = 0;
    let mut name = slug.to_string();
    let mut created_at = None;
    let mut updated_at = None;

    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let header = &rest[..end];
            body_start = 4 + end + 4;
            for line in header.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Some((k, v)) = trimmed.split_once(':') else {
                    continue;
                };
                match k.trim() {
                    "name" => name = unquote(v.trim()),
                    "created_at" => created_at = Some(unquote(v.trim())),
                    "updated_at" => updated_at = Some(unquote(v.trim())),
                    _ => {}
                }
            }
        }
    }

    let body = raw
        .get(body_start..)
        .unwrap_or("")
        .trim_start_matches('\n')
        .to_string();

    LibraryEntry {
        slug: slug.to_string(),
        name,
        created_at,
        updated_at,
        body,
    }
}

fn render_entry(entry: &LibraryEntry) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("name: \"{}\"\n", escape_quote(&entry.name)));
    if let Some(created_at) = &entry.created_at {
        out.push_str(&format!("created_at: \"{}\"\n", escape_quote(created_at)));
    }
    if let Some(updated_at) = &entry.updated_at {
        out.push_str(&format!("updated_at: \"{}\"\n", escape_quote(updated_at)));
    }
    out.push_str("---\n");
    out.push_str(entry.body.trim_start_matches('\n'));
    out
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        return s[1..s.len() - 1].replace("\\\"", "\"");
    }
    s.to_string()
}

fn escape_quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn slug_sanitise_basics() {
        assert_eq!(sanitise_slug("Hello World"), Some("Hello-World".into()));
        assert_eq!(sanitise_slug("keep-me"), Some("keep-me".into()));
        assert_eq!(sanitise_slug("  trim  "), Some("trim".into()));
        assert_eq!(sanitise_slug("slash/bad"), Some("slashbad".into()));
        assert_eq!(sanitise_slug(""), None);
        assert_eq!(sanitise_slug(".hidden"), None);
        assert_eq!(sanitise_slug("..escape"), None);
    }

    #[tokio::test]
    async fn save_list_read_roundtrip() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        save(
            &root,
            LibraryKind::References,
            "ticket-order",
            SaveInput {
                name: "Ticket order".into(),
                body: "- 43\n- 48\n- 49".into(),
            },
        )
        .await
        .unwrap();

        let list = list(&root, LibraryKind::References).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Ticket order");
        assert_eq!(list[0].body, "- 43\n- 48\n- 49");

        let fetched = read(&root, LibraryKind::References, "ticket-order")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.name, "Ticket order");
    }

    #[tokio::test]
    async fn overwrite_preserves_created_at_and_updates_timestamp() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let first = save(
            &root,
            LibraryKind::Prompts,
            "commit-and-push",
            SaveInput {
                name: "Commit and push".into(),
                body: "commit everything".into(),
            },
        )
        .await
        .unwrap();

        let second = save(
            &root,
            LibraryKind::Prompts,
            "commit-and-push",
            SaveInput {
                name: "Commit and push".into(),
                body: "commit everything with git diff summary".into(),
            },
        )
        .await
        .unwrap();

        assert_eq!(second.created_at, first.created_at);
        assert_ne!(second.updated_at, first.updated_at);
        assert_eq!(second.body, "commit everything with git diff summary");
    }

    #[tokio::test]
    async fn delete_removes_file() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        save(
            &root,
            LibraryKind::References,
            "to-delete",
            SaveInput {
                name: "bye".into(),
                body: "gone soon".into(),
            },
        )
        .await
        .unwrap();

        assert!(delete(&root, LibraryKind::References, "to-delete")
            .await
            .unwrap());
        assert!(read(&root, LibraryKind::References, "to-delete")
            .await
            .unwrap()
            .is_none());
        assert!(!delete(&root, LibraryKind::References, "to-delete")
            .await
            .unwrap());
    }
}
