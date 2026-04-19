//! Per-repo library: references (pinned assistant outputs) and
//! prompts (saved user inputs). Both are markdown files on disk with
//! a simple YAML-ish frontmatter block. Keeping them as plain files
//! means:
//!
//!   - User can `git add` / version the library without any schema
//!     or tooling
//!   - Survives a backend restart trivially
//!   - Editable with any text editor
//!   - No migration if the fields change (we just tolerate unknown
//!     keys when reading)
//!
//! Layout: `<repo_root>/.shuttlecraft/<kind>/<slug>.md` where `<kind>`
//! is `refs` or `prompts`. Slugs are user-chosen and sanitised
//! (alphanumeric + hyphen + underscore + dot).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryKind {
    Refs,
    Prompts,
}

impl LibraryKind {
    /// Kind name as used in URL paths and the on-disk directory.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "refs" => Some(LibraryKind::Refs),
            "prompts" => Some(LibraryKind::Prompts),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            LibraryKind::Refs => "refs",
            LibraryKind::Prompts => "prompts",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LibraryEntry {
    pub slug: String,
    pub name: String,
    pub tags: Vec<String>,
    pub created_at: Option<String>,
    /// Body text (without the frontmatter). For refs this is the
    /// captured markdown; for prompts it's the user's prompt text.
    pub body: String,
    /// Arbitrary additional frontmatter fields we didn't recognise.
    /// Carried through verbatim on write so user-added keys survive.
    #[serde(default)]
    pub extras: serde_json::Map<String, serde_json::Value>,
}

/// Sanitise a user-chosen slug to safe filesystem characters. Rejects
/// anything that could escape the kind subdirectory.
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
        // Any other character silently drops.
    }
    // Collapse multiple hyphens.
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() || out.starts_with('.') {
        return None;
    }
    Some(out)
}

fn library_dir(repo_root: &Path, kind: LibraryKind) -> PathBuf {
    repo_root.join(".shuttlecraft").join(kind.as_str())
}

fn entry_path(repo_root: &Path, kind: LibraryKind, slug: &str) -> PathBuf {
    library_dir(repo_root, kind).join(format!("{slug}.md"))
}

pub async fn list(repo_root: &Path, kind: LibraryKind) -> anyhow::Result<Vec<LibraryEntry>> {
    let dir = library_dir(repo_root, kind);
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
            Ok(e) => entries.push(e),
            Err(err) => {
                tracing::warn!(%err, path = %full.display(), "library read failed");
            }
        }
    }
    entries.sort_by(|a, b| {
        b.created_at
            .as_deref()
            .unwrap_or("")
            .cmp(a.created_at.as_deref().unwrap_or(""))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

pub async fn read(
    repo_root: &Path,
    kind: LibraryKind,
    slug: &str,
) -> anyhow::Result<Option<LibraryEntry>> {
    let slug = match sanitise_slug(slug) {
        Some(s) => s,
        None => return Ok(None),
    };
    let path = entry_path(repo_root, kind, &slug);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(read_file(&path, &slug).await?))
}

async fn read_file(path: &Path, slug: &str) -> anyhow::Result<LibraryEntry> {
    let raw = tokio::fs::read_to_string(path).await?;
    Ok(parse_entry(&raw, slug))
}

pub async fn save(
    repo_root: &Path,
    kind: LibraryKind,
    slug: &str,
    entry: SaveInput,
) -> anyhow::Result<LibraryEntry> {
    let slug = sanitise_slug(slug).ok_or_else(|| anyhow::anyhow!("invalid slug"))?;
    let dir = library_dir(repo_root, kind);
    tokio::fs::create_dir_all(&dir).await?;
    let path = entry_path(repo_root, kind, &slug);

    // If the file already existed, read its extras and created_at so
    // we preserve user-added frontmatter across overwrites.
    let existing = if path.exists() {
        read_file(&path, &slug).await.ok()
    } else {
        None
    };
    let created_at = existing
        .as_ref()
        .and_then(|e| e.created_at.clone())
        .or(Some(chrono::Utc::now().to_rfc3339()));
    let extras = existing
        .as_ref()
        .map(|e| e.extras.clone())
        .unwrap_or_default();

    let full_entry = LibraryEntry {
        slug: slug.clone(),
        name: entry.name,
        tags: entry.tags,
        created_at,
        body: entry.body,
        extras,
    };
    let rendered = render_entry(&full_entry);
    tokio::fs::write(&path, rendered).await?;
    Ok(full_entry)
}

pub async fn delete(repo_root: &Path, kind: LibraryKind, slug: &str) -> anyhow::Result<bool> {
    let slug = match sanitise_slug(slug) {
        Some(s) => s,
        None => return Ok(false),
    };
    let path = entry_path(repo_root, kind, &slug);
    if !path.exists() {
        return Ok(false);
    }
    tokio::fs::remove_file(&path).await?;
    Ok(true)
}

#[derive(Debug, Deserialize)]
pub struct SaveInput {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub body: String,
}

// ─── frontmatter format ──────────────────────────────────────────────

fn parse_entry(raw: &str, slug: &str) -> LibraryEntry {
    // Tiny parser. Recognises:
    //   ---
    //   name: "quoted or bare"
    //   tags: [a, b, "c"]
    //   created_at: ...
    //   other: value   ← captured into `extras` as a JSON string
    //   ---
    //   <body>
    //
    // We don't try to be a full YAML parser; real frontmatter in a
    // library file stays simple.
    let mut body_start = 0;
    let mut name = slug.to_string();
    let mut tags: Vec<String> = Vec::new();
    let mut created_at: Option<String> = None;
    let mut extras: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let header = &rest[..end];
            // advance body_start past `---\n...\n---\n`
            body_start = 4 + end + 4; // "---\n" + header + "\n---\n"
            for line in header.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Some((k, v)) = trimmed.split_once(':') else {
                    continue;
                };
                let key = k.trim().to_string();
                let value = v.trim().to_string();
                match key.as_str() {
                    "name" => name = unquote(&value),
                    "tags" => tags = parse_tags_list(&value),
                    "created_at" => created_at = Some(unquote(&value)),
                    _ => {
                        extras.insert(key, serde_json::Value::String(unquote(&value)));
                    }
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
        tags,
        created_at,
        body,
        extras,
    }
}

fn render_entry(entry: &LibraryEntry) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("name: \"{}\"\n", escape_quote(&entry.name)));
    if let Some(ts) = &entry.created_at {
        out.push_str(&format!("created_at: \"{}\"\n", escape_quote(ts)));
    }
    if !entry.tags.is_empty() {
        let joined: Vec<String> = entry
            .tags
            .iter()
            .map(|t| format!("\"{}\"", escape_quote(t)))
            .collect();
        out.push_str(&format!("tags: [{}]\n", joined.join(", ")));
    }
    for (k, v) in &entry.extras {
        if matches!(k.as_str(), "name" | "created_at" | "tags") {
            continue;
        }
        if let Some(s) = v.as_str() {
            out.push_str(&format!("{k}: \"{}\"\n", escape_quote(s)));
        }
    }
    out.push_str("---\n");
    out.push_str(entry.body.trim_start_matches('\n'));
    if !out.ends_with('\n') {
        out.push('\n');
    }
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

fn parse_tags_list(raw: &str) -> Vec<String> {
    let s = raw.trim().trim_start_matches('[').trim_end_matches(']');
    s.split(',')
        .map(|t| unquote(t.trim()))
        .filter(|t| !t.is_empty())
        .collect()
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
        let repo = tmp.path().to_path_buf();
        save(
            &repo,
            LibraryKind::Refs,
            "first-pin",
            SaveInput {
                name: "First Pin".into(),
                tags: vec!["design".into(), "ticket".into()],
                body: "# ticket list\n\n- a\n- b\n".into(),
            },
        )
        .await
        .unwrap();

        let list = list(&repo, LibraryKind::Refs).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "First Pin");
        assert_eq!(list[0].tags, vec!["design", "ticket"]);
        assert!(list[0].body.contains("# ticket list"));

        let fetched = read(&repo, LibraryKind::Refs, "first-pin").await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().name, "First Pin");
    }

    #[tokio::test]
    async fn overwrite_preserves_created_at_and_extras() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let first = save(
            &repo,
            LibraryKind::Prompts,
            "commit-style",
            SaveInput {
                name: "Commit style".into(),
                tags: vec![],
                body: "body v1".into(),
            },
        )
        .await
        .unwrap();
        let created_at = first.created_at.clone();

        // Hand-edit a custom field into the file to simulate a
        // user-added frontmatter key.
        let path = entry_path(&repo, LibraryKind::Prompts, "commit-style");
        let raw = tokio::fs::read_to_string(&path).await.unwrap();
        let patched = raw.replacen("---\n", "---\nsource: \"manual\"\n", 1);
        tokio::fs::write(&path, patched).await.unwrap();

        let second = save(
            &repo,
            LibraryKind::Prompts,
            "commit-style",
            SaveInput {
                name: "Commit style".into(),
                tags: vec![],
                body: "body v2".into(),
            },
        )
        .await
        .unwrap();

        assert_eq!(second.body, "body v2");
        assert_eq!(second.created_at, created_at, "created_at must survive");
        assert_eq!(
            second.extras.get("source").and_then(|v| v.as_str()),
            Some("manual"),
            "user-added frontmatter keys must survive"
        );
    }

    #[tokio::test]
    async fn delete_removes_file() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        save(
            &repo,
            LibraryKind::Refs,
            "to-delete",
            SaveInput {
                name: "bye".into(),
                tags: vec![],
                body: "gone soon".into(),
            },
        )
        .await
        .unwrap();
        assert!(delete(&repo, LibraryKind::Refs, "to-delete").await.unwrap());
        assert!(read(&repo, LibraryKind::Refs, "to-delete")
            .await
            .unwrap()
            .is_none());
        // Second delete is a no-op.
        assert!(!delete(&repo, LibraryKind::Refs, "to-delete").await.unwrap());
    }
}
