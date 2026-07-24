use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Pool;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    Working,
    AwaitingPrompt,
    NeedsInput,
    Blocked,
    Unknown,
}

impl ActivityState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::AwaitingPrompt => "awaiting_prompt",
            Self::NeedsInput => "needs_input",
            Self::Blocked => "blocked",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ActivityRecord {
    pub pty_session_id: Uuid,
    pub state: String,
    pub summary: Option<String>,
    pub reason: Option<String>,
    pub source: String,
    pub confidence: String,
    pub updated_at: DateTime<Utc>,
}

pub async fn set(
    pool: &Pool,
    pty_session_id: Uuid,
    state: ActivityState,
    summary: Option<&str>,
    reason: Option<&str>,
    source: &str,
    confidence: &str,
) -> anyhow::Result<ActivityRecord> {
    validate_source(source)?;
    validate_confidence(confidence)?;
    let summary = clean_optional(summary, 240);
    let reason = clean_optional(reason, 500);
    let record: Option<ActivityRecord> = sqlx::query_as(
        "INSERT INTO session_activity_state \
             (pty_session_id, state, summary, reason, source, confidence, updated_at) \
         SELECT ps.id, $2, $3, $4, $5, $6, NOW() \
           FROM pty_sessions ps \
          WHERE ps.id = $1 AND ps.state = 'live' \
         ON CONFLICT (pty_session_id) DO UPDATE \
           SET state = EXCLUDED.state, summary = EXCLUDED.summary, \
               reason = EXCLUDED.reason, source = EXCLUDED.source, \
               confidence = EXCLUDED.confidence, updated_at = NOW() \
         WHERE NOT ( \
             session_activity_state.source = 'agent' \
             AND session_activity_state.state IN ('needs_input', 'blocked') \
             AND EXCLUDED.state = 'awaiting_prompt' \
         ) \
         RETURNING pty_session_id, state, summary, reason, source, confidence, updated_at",
    )
    .bind(pty_session_id)
    .bind(state.as_str())
    .bind(summary)
    .bind(reason)
    .bind(source)
    .bind(confidence)
    .fetch_optional(pool)
    .await?;
    if let Some(record) = record {
        return Ok(record);
    }
    sqlx::query_as(
        "SELECT pty_session_id, state, summary, reason, source, confidence, updated_at \
           FROM session_activity_state WHERE pty_session_id = $1",
    )
    .bind(pty_session_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| anyhow::anyhow!("live PTY session not found: {pty_session_id}"))
}

pub async fn clear(pool: &Pool, pty_session_id: Uuid) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM session_activity_state WHERE pty_session_id = $1")
        .bind(pty_session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_for_current_agent_session(
    pool: &Pool,
    agent_session_uuid: Uuid,
    state: ActivityState,
    summary: Option<&str>,
    reason: Option<&str>,
    source: &str,
    confidence: &str,
) -> anyhow::Result<bool> {
    validate_source(source)?;
    validate_confidence(confidence)?;
    let summary = clean_optional(summary, 240);
    let reason = clean_optional(reason, 500);
    let result = sqlx::query(
        "INSERT INTO session_activity_state \
             (pty_session_id, state, summary, reason, source, confidence, updated_at) \
         SELECT ps.id, $2, $3, $4, $5, $6, NOW() \
           FROM pty_sessions ps \
          WHERE ps.current_session_uuid = $1 \
            AND ps.state = 'live' \
            AND ps.agent_runtime_state IN ('starting', 'running') \
         ON CONFLICT (pty_session_id) DO UPDATE \
           SET state = EXCLUDED.state, summary = EXCLUDED.summary, \
               reason = EXCLUDED.reason, source = EXCLUDED.source, \
               confidence = EXCLUDED.confidence, updated_at = NOW() \
         WHERE NOT ( \
             session_activity_state.source = 'agent' \
             AND session_activity_state.state IN ('needs_input', 'blocked') \
             AND EXCLUDED.state = 'awaiting_prompt' \
         )",
    )
    .bind(agent_session_uuid)
    .bind(state.as_str())
    .bind(summary)
    .bind(reason)
    .bind(source)
    .bind(confidence)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

fn clean_optional(value: Option<&str>, max_chars: usize) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.chars().take(max_chars).collect())
}

fn validate_source(source: &str) -> anyhow::Result<()> {
    if matches!(source, "launcher" | "hook" | "ingester" | "agent" | "user") {
        Ok(())
    } else {
        anyhow::bail!("invalid activity source: {source}")
    }
}

fn validate_confidence(confidence: &str) -> anyhow::Result<()> {
    if matches!(confidence, "explicit" | "derived" | "unknown") {
        Ok(())
    } else {
        anyhow::bail!("invalid activity confidence: {confidence}")
    }
}
