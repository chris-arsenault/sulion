//! Session-scoped deferred prompts for the currently correlated agent
//! invocation. Unlike the reusable prompt library, these entries are
//! one-off follow-ups tied to a specific transcript session UUID.
//!
//! One row per prompt in `future_prompts`, keyed `(session_uuid, id)`.
//! Formerly markdown files under a node-local directory; database rows
//! are what let any process that answers the API see the same entries.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Pool;

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

type EntryRow = (String, String, DateTime<Utc>, DateTime<Utc>, String);

fn entry_from_row((id, state, created_at, updated_at, text): EntryRow) -> FuturePromptEntry {
    FuturePromptEntry {
        id,
        state: FuturePromptState::parse(&state).unwrap_or(FuturePromptState::Pending),
        created_at: Some(created_at.to_rfc3339()),
        updated_at: Some(updated_at.to_rfc3339()),
        text,
    }
}

pub async fn list(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<Vec<FuturePromptEntry>> {
    let rows: Vec<EntryRow> = sqlx::query_as(
        "SELECT id, state, created_at, updated_at, text \
           FROM future_prompts \
          WHERE session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_all(pool)
    .await?;
    let mut entries: Vec<FuturePromptEntry> = rows.into_iter().map(entry_from_row).collect();

    // Pending first, oldest first (send order); everything else newest
    // first (recent history on top).
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

/// Cheap companion to `list` — returns only how many entries are in
/// the `pending` state. Used by `/api/app-state` to power the sidebar
/// badge without materialising every prompt body.
pub async fn count_pending(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<usize> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM future_prompts \
          WHERE session_uuid = $1 AND state = 'pending'",
    )
    .bind(session_uuid)
    .fetch_one(pool)
    .await?;
    Ok(count.max(0) as usize)
}

pub async fn create(
    pool: &Pool,
    session_uuid: Uuid,
    input: CreateInput,
) -> anyhow::Result<FuturePromptEntry> {
    let text = input.text.trim().to_string();
    if text.is_empty() {
        anyhow::bail!("text must not be empty");
    }
    let id = Uuid::new_v4().to_string();
    let row: EntryRow = sqlx::query_as(
        "INSERT INTO future_prompts (session_uuid, id, state, text) \
         VALUES ($1, $2, 'pending', $3) \
         RETURNING id, state, created_at, updated_at, text",
    )
    .bind(session_uuid)
    .bind(&id)
    .bind(&text)
    .fetch_one(pool)
    .await?;
    Ok(entry_from_row(row))
}

pub async fn update(
    pool: &Pool,
    session_uuid: Uuid,
    id: &str,
    input: UpdateInput,
) -> anyhow::Result<Option<FuturePromptEntry>> {
    let id = match sanitise_id(id) {
        Some(id) => id,
        None => return Ok(None),
    };
    if let Some(text) = &input.text {
        if text.trim().is_empty() {
            anyhow::bail!("text must not be empty");
        }
    }
    let row: Option<EntryRow> = sqlx::query_as(
        "UPDATE future_prompts SET \
             text = COALESCE($3, text), \
             state = COALESCE($4, state), \
             updated_at = now() \
          WHERE session_uuid = $1 AND id = $2 \
          RETURNING id, state, created_at, updated_at, text",
    )
    .bind(session_uuid)
    .bind(&id)
    .bind(input.text.as_ref().map(|text| text.trim().to_string()))
    .bind(input.state.map(FuturePromptState::as_str))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(entry_from_row))
}

pub async fn delete(pool: &Pool, session_uuid: Uuid, id: &str) -> anyhow::Result<bool> {
    let id = match sanitise_id(id) {
        Some(id) => id,
        None => return Ok(false),
    };
    let deleted = sqlx::query("DELETE FROM future_prompts WHERE session_uuid = $1 AND id = $2")
        .bind(session_uuid)
        .bind(&id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(deleted > 0)
}

fn state_rank(state: FuturePromptState) -> u8 {
    match state {
        FuturePromptState::Pending => 0,
        FuturePromptState::Sent => 1,
    }
}
