//! Global prompt/reference library, stored in Postgres. These entries
//! are intentionally lightweight:
//!
//! - prompts are reusable instructions the user injects into the
//!   active terminal
//! - references are saved assistant outputs the user wants to revisit
//!
//! One row per entry in `library_entries`, keyed `(kind, slug)`. The
//! library predates the split topology as markdown files under a
//! node-local directory; database rows are what let any process that
//! answers `/api/library` see the same entries.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::Pool;

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

type EntryRow = (String, String, DateTime<Utc>, DateTime<Utc>, String);

fn entry_from_row((slug, name, created_at, updated_at, body): EntryRow) -> LibraryEntry {
    LibraryEntry {
        slug,
        name,
        created_at: Some(created_at.to_rfc3339()),
        updated_at: Some(updated_at.to_rfc3339()),
        body,
    }
}

pub async fn next_available_slug(
    pool: &Pool,
    kind: LibraryKind,
    desired: &str,
) -> anyhow::Result<String> {
    let mut candidate = desired.to_string();
    let mut suffix = 2;
    loop {
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM library_entries WHERE kind = $1 AND slug = $2)",
        )
        .bind(kind.as_str())
        .bind(&candidate)
        .fetch_one(pool)
        .await?;
        if !taken {
            return Ok(candidate);
        }
        candidate = format!("{desired}-{suffix}");
        suffix += 1;
    }
}

pub async fn list(pool: &Pool, kind: LibraryKind) -> anyhow::Result<Vec<LibraryEntry>> {
    let rows: Vec<EntryRow> = sqlx::query_as(
        "SELECT slug, name, created_at, updated_at, body \
           FROM library_entries \
          WHERE kind = $1 \
          ORDER BY updated_at DESC, created_at DESC, name ASC",
    )
    .bind(kind.as_str())
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(entry_from_row).collect())
}

pub async fn read(
    pool: &Pool,
    kind: LibraryKind,
    slug: &str,
) -> anyhow::Result<Option<LibraryEntry>> {
    let slug = match sanitise_slug(slug) {
        Some(slug) => slug,
        None => return Ok(None),
    };
    let row: Option<EntryRow> = sqlx::query_as(
        "SELECT slug, name, created_at, updated_at, body \
           FROM library_entries \
          WHERE kind = $1 AND slug = $2",
    )
    .bind(kind.as_str())
    .bind(&slug)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(entry_from_row))
}

pub async fn save(
    pool: &Pool,
    kind: LibraryKind,
    slug: &str,
    entry: SaveInput,
) -> anyhow::Result<LibraryEntry> {
    let slug = sanitise_slug(slug).ok_or_else(|| anyhow::anyhow!("invalid slug"))?;
    let row: EntryRow = sqlx::query_as(
        "INSERT INTO library_entries (kind, slug, name, body) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (kind, slug) DO UPDATE SET \
             name = EXCLUDED.name, \
             body = EXCLUDED.body, \
             updated_at = now() \
         RETURNING slug, name, created_at, updated_at, body",
    )
    .bind(kind.as_str())
    .bind(&slug)
    .bind(entry.name.trim())
    .bind(&entry.body)
    .fetch_one(pool)
    .await?;
    Ok(entry_from_row(row))
}

pub async fn delete(pool: &Pool, kind: LibraryKind, slug: &str) -> anyhow::Result<bool> {
    let slug = match sanitise_slug(slug) {
        Some(slug) => slug,
        None => return Ok(false),
    };
    let deleted = sqlx::query("DELETE FROM library_entries WHERE kind = $1 AND slug = $2")
        .bind(kind.as_str())
        .bind(&slug)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(deleted > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
