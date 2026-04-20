//! Session-scoped deferred prompts for the currently correlated agent
//! invocation. Unlike the reusable prompt library, these entries are
//! one-off follow-ups tied to a specific transcript session UUID.
//!
//! Layout: `<future_prompts_root>/<session_uuid>/<id>.md`

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuturePromptState {
    Pending,
    Sent,
}

impl FuturePromptState {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "sent" => Some(Self::Sent),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sent => "sent",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuturePromptEntry {
    pub id: String,
    pub state: FuturePromptState,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateInput {
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateInput {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub state: Option<FuturePromptState>,
}

fn session_dir(root: &Path, session_uuid: Uuid) -> PathBuf {
    root.join(session_uuid.to_string())
}

fn entry_path(root: &Path, session_uuid: Uuid, id: &str) -> PathBuf {
    session_dir(root, session_uuid).join(format!("{id}.md"))
}

fn sanitise_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(trimmed.len());
    for c in trimmed.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        }
    }
    if out.is_empty() || out.starts_with('.') {
        return None;
    }
    Some(out)
}

pub async fn list(root: &Path, session_uuid: Uuid) -> anyhow::Result<Vec<FuturePromptEntry>> {
    let dir = session_dir(root, session_uuid);
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
        let id = file_name.trim_end_matches(".md").to_string();
        let full = entry.path();
        match read_file(&full, &id).await {
            Ok(entry) => entries.push(entry),
            Err(err) => tracing::warn!(%err, path = %full.display(), "future prompt read failed"),
        }
    }

    entries.sort_by(|a, b| {
        state_rank(a.state)
            .cmp(&state_rank(b.state))
            .then_with(|| {
                if a.state == FuturePromptState::Pending && b.state == FuturePromptState::Pending {
                    a.created_at
                        .as_deref()
                        .unwrap_or("")
                        .cmp(b.created_at.as_deref().unwrap_or(""))
                } else {
                    b.updated_at
                        .as_deref()
                        .unwrap_or("")
                        .cmp(a.updated_at.as_deref().unwrap_or(""))
                }
            })
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(entries)
}

pub async fn create(
    root: &Path,
    session_uuid: Uuid,
    input: CreateInput,
) -> anyhow::Result<FuturePromptEntry> {
    let text = input.text.trim().to_string();
    if text.is_empty() {
        anyhow::bail!("text must not be empty");
    }

    let dir = session_dir(root, session_uuid);
    tokio::fs::create_dir_all(&dir).await?;
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let entry = FuturePromptEntry {
        id: id.clone(),
        state: FuturePromptState::Pending,
        created_at: Some(now.clone()),
        updated_at: Some(now),
        text,
    };

    tokio::fs::write(entry_path(root, session_uuid, &id), render_entry(&entry)).await?;
    Ok(entry)
}

pub async fn update(
    root: &Path,
    session_uuid: Uuid,
    id: &str,
    input: UpdateInput,
) -> anyhow::Result<Option<FuturePromptEntry>> {
    let id = match sanitise_id(id) {
        Some(id) => id,
        None => return Ok(None),
    };
    let path = entry_path(root, session_uuid, &id);
    if !path.exists() {
        return Ok(None);
    }

    let mut entry = read_file(&path, &id).await?;
    if let Some(text) = input.text {
        let text = text.trim().to_string();
        if text.is_empty() {
            anyhow::bail!("text must not be empty");
        }
        entry.text = text;
    }
    if let Some(state) = input.state {
        entry.state = state;
    }
    entry.updated_at = Some(chrono::Utc::now().to_rfc3339());

    tokio::fs::write(&path, render_entry(&entry)).await?;
    Ok(Some(entry))
}

pub async fn delete(root: &Path, session_uuid: Uuid, id: &str) -> anyhow::Result<bool> {
    let id = match sanitise_id(id) {
        Some(id) => id,
        None => return Ok(false),
    };
    let path = entry_path(root, session_uuid, &id);
    if !path.exists() {
        return Ok(false);
    }
    tokio::fs::remove_file(path).await?;
    Ok(true)
}

async fn read_file(path: &Path, id: &str) -> anyhow::Result<FuturePromptEntry> {
    let raw = tokio::fs::read_to_string(path).await?;
    Ok(parse_entry(&raw, id))
}

fn parse_entry(raw: &str, id: &str) -> FuturePromptEntry {
    let mut body_start = 0;
    let mut state = FuturePromptState::Pending;
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
                    "state" => {
                        if let Some(parsed) = FuturePromptState::parse(unquote(v.trim()).as_str()) {
                            state = parsed;
                        }
                    }
                    "created_at" => created_at = Some(unquote(v.trim())),
                    "updated_at" => updated_at = Some(unquote(v.trim())),
                    _ => {}
                }
            }
        }
    }

    FuturePromptEntry {
        id: id.to_string(),
        state,
        created_at,
        updated_at,
        text: raw
            .get(body_start..)
            .unwrap_or("")
            .trim_start_matches('\n')
            .to_string(),
    }
}

fn render_entry(entry: &FuturePromptEntry) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("state: \"{}\"\n", entry.state.as_str()));
    if let Some(created_at) = &entry.created_at {
        out.push_str(&format!("created_at: \"{}\"\n", escape_quote(created_at)));
    }
    if let Some(updated_at) = &entry.updated_at {
        out.push_str(&format!("updated_at: \"{}\"\n", escape_quote(updated_at)));
    }
    out.push_str("---\n");
    out.push_str(entry.text.trim_start_matches('\n'));
    out
}

fn state_rank(state: FuturePromptState) -> u8 {
    match state {
        FuturePromptState::Pending => 0,
        FuturePromptState::Sent => 1,
    }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        s[1..s.len() - 1].replace("\\\"", "\"")
    } else {
        s.to_string()
    }
}

fn escape_quote(s: &str) -> String {
    s.replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_list_update_delete_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let session_uuid = Uuid::new_v4();

        let created = create(
            root.path(),
            session_uuid,
            CreateInput {
                text: "follow up later".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(created.state, FuturePromptState::Pending);

        let listed = list(root.path(), session_uuid).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].text, "follow up later");

        let updated = update(
            root.path(),
            session_uuid,
            &created.id,
            UpdateInput {
                text: Some("send this next".into()),
                state: Some(FuturePromptState::Sent),
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(updated.text, "send this next");
        assert_eq!(updated.state, FuturePromptState::Sent);

        assert!(delete(root.path(), session_uuid, &created.id)
            .await
            .unwrap());
        assert!(list(root.path(), session_uuid).await.unwrap().is_empty());
    }
}
