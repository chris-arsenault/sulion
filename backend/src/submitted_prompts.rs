//! Prompts submitted from the timeline input, recorded before their bytes
//! reach the PTY.
//!
//! A harness sitting on a startup dialog swallows typed text without
//! leaving a transcript trace, so the transcript cannot be the record of
//! what the user asked. One row per submission in `submitted_prompts`.
//! [`reconcile`] matches open rows against projected timeline turns in the
//! PTY's transcript sessions; a row that never matches stays listed until
//! the user dismisses it.
//!
//! The same module answers whether the timeline input should be open at
//! all: [`prompt_gate`] reports a harness that has not yet reported a
//! session for its current launch, or one that is waiting on a terminal
//! dialog, so typed text is not fed into a screen that cannot take it.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::db::Pool;

/// Rows returned by [`list`]: enough history to find a prompt from earlier
/// in the day without paging.
pub const RECENT_LIMIT: i64 = 50;

/// How often the control process re-runs matching for open rows.
pub const RECONCILE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Matched and dismissed rows older than this are dropped by [`prune`].
const RETENTION_DAYS: i32 = 7;

/// Why the timeline input is closed for a PTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptGate {
    /// The harness is running but has not reported a session for this
    /// launch: it is still on its startup screens.
    Starting,
    NeedsInput,
    Blocked,
}

impl PromptGate {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Starting => {
                "the agent has not reported a session for this launch yet; answer any startup prompt in the terminal"
            }
            Self::NeedsInput => "the agent is waiting for input in the terminal",
            Self::Blocked => "the agent reports it is blocked",
        }
    }
}

/// A correlation counts for the current launch only when it landed at or
/// after the launch started. `current_session_uuid` is sticky across agent
/// invocations in one PTY, so on its own it says nothing about this one.
pub fn correlated_for_launch(
    current_session_uuid: Option<Uuid>,
    correlated_at: Option<DateTime<Utc>>,
    runtime_started_at: Option<DateTime<Utc>>,
) -> bool {
    if current_session_uuid.is_none() {
        return false;
    }
    match (correlated_at, runtime_started_at) {
        (Some(correlated), Some(started)) => correlated >= started,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// Whether a running harness is past its startup screens. Launch-scoped
/// correlation is the primary signal; activity the harness itself reported
/// (its hooks, the `sulion activity` CLI, or its transcript) is equally
/// conclusive, and covers a launch whose session hook never arrived. The
/// launcher's own `unknown` marker and the prompt route's `user` write are
/// not evidence: both happen without the harness having accepted anything.
pub fn harness_ready(
    current_session_uuid: Option<Uuid>,
    correlated_at: Option<DateTime<Utc>>,
    runtime_started_at: Option<DateTime<Utc>>,
    activity_source: Option<&str>,
) -> bool {
    correlated_for_launch(current_session_uuid, correlated_at, runtime_started_at)
        || matches!(activity_source, Some("hook" | "agent" | "ingester"))
}

#[derive(sqlx::FromRow)]
struct GateRow {
    agent_runtime_state: String,
    agent_runtime_started_at: Option<DateTime<Utc>>,
    current_session_uuid: Option<Uuid>,
    current_session_correlated_at: Option<DateTime<Utc>>,
    activity_state: Option<String>,
    activity_source: Option<String>,
}

pub async fn prompt_gate(pool: &Pool, pty_id: Uuid) -> anyhow::Result<Option<PromptGate>> {
    let row: Option<GateRow> = sqlx::query_as(
        "SELECT ps.agent_runtime_state, ps.agent_runtime_started_at, \
                ps.current_session_uuid, ps.current_session_correlated_at, \
                sas.state AS activity_state, sas.source AS activity_source \
           FROM pty_sessions ps \
           LEFT JOIN session_activity_state sas ON sas.pty_session_id = ps.id \
          WHERE ps.id = $1",
    )
    .bind(pty_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.agent_runtime_state == "running"
        && !harness_ready(
            row.current_session_uuid,
            row.current_session_correlated_at,
            row.agent_runtime_started_at,
            row.activity_source.as_deref(),
        )
    {
        return Ok(Some(PromptGate::Starting));
    }
    Ok(match row.activity_state.as_deref() {
        Some("needs_input") => Some(PromptGate::NeedsInput),
        Some("blocked") => Some(PromptGate::Blocked),
        _ => None,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct SubmittedPrompt {
    pub id: Uuid,
    pub agent: Option<String>,
    pub text: String,
    pub forced: bool,
    pub submitted_at: DateTime<Utc>,
    /// `matched`, `unmatched`, `failed`, or `dismissed`.
    pub state: &'static str,
    pub delivery_error: Option<String>,
    /// `<session_uuid>:<turn_id>` of the timeline turn this prompt became.
    pub matched_turn_key: Option<String>,
    pub matched_at: Option<DateTime<Utc>>,
    pub dismissed_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct PromptRow {
    id: Uuid,
    agent: Option<String>,
    text: String,
    forced: bool,
    submitted_at: DateTime<Utc>,
    delivery_error: Option<String>,
    matched_session_uuid: Option<Uuid>,
    matched_turn_id: Option<i64>,
    matched_at: Option<DateTime<Utc>>,
    dismissed_at: Option<DateTime<Utc>>,
}

impl From<PromptRow> for SubmittedPrompt {
    fn from(row: PromptRow) -> Self {
        let state = if row.dismissed_at.is_some() {
            "dismissed"
        } else if row.matched_at.is_some() {
            "matched"
        } else if row.delivery_error.is_some() {
            "failed"
        } else {
            "unmatched"
        };
        let matched_turn_key = match (row.matched_session_uuid, row.matched_turn_id) {
            (Some(session), Some(turn)) => Some(format!("{session}:{turn}")),
            _ => None,
        };
        Self {
            id: row.id,
            agent: row.agent,
            text: row.text,
            forced: row.forced,
            submitted_at: row.submitted_at,
            state,
            delivery_error: row.delivery_error,
            matched_turn_key,
            matched_at: row.matched_at,
            dismissed_at: row.dismissed_at,
        }
    }
}

/// Store the prompt before anything is written to the PTY. Returns the row id
/// so a failed delivery can be recorded against it.
pub async fn record(
    pool: &Pool,
    pty_id: Uuid,
    agent: Option<&str>,
    text: &str,
    forced: bool,
) -> anyhow::Result<Uuid> {
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO submitted_prompts (pty_session_id, agent, text, forced) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(pty_id)
    .bind(agent)
    .bind(text)
    .bind(forced)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn mark_delivery_error(pool: &Pool, id: Uuid, error: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE submitted_prompts SET delivery_error = $2 WHERE id = $1")
        .bind(id)
        .bind(error)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list(pool: &Pool, pty_id: Uuid) -> anyhow::Result<Vec<SubmittedPrompt>> {
    let rows: Vec<PromptRow> = sqlx::query_as(
        "SELECT id, agent, text, forced, submitted_at, delivery_error, \
                matched_session_uuid, matched_turn_id, matched_at, dismissed_at \
           FROM submitted_prompts \
          WHERE pty_session_id = $1 \
          ORDER BY submitted_at DESC \
          LIMIT $2",
    )
    .bind(pty_id)
    .bind(RECENT_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(SubmittedPrompt::from).collect())
}

pub async fn dismiss(pool: &Pool, pty_id: Uuid, id: Uuid) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE submitted_prompts SET dismissed_at = NOW() \
          WHERE id = $1 AND pty_session_id = $2 AND dismissed_at IS NULL",
    )
    .bind(id)
    .bind(pty_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Match open rows against projected timeline turns. A turn qualifies when it
/// belongs to a transcript session correlated to the prompt's PTY, started no
/// earlier than a few seconds before the submission (clock skew between the
/// browser round trip and the harness's own timestamp), carries the same
/// whitespace-normalized prompt text, and is not already claimed. The earliest
/// qualifying turn wins, and one turn claims at most one prompt per pass, so
/// two identical submissions resolve to two turns across passes rather than
/// sharing one.
///
/// Returns the number of rows matched.
pub async fn reconcile(pool: &Pool) -> anyhow::Result<u64> {
    let result = sqlx::query(
        "WITH open AS ( \
            SELECT sp.id, sp.pty_session_id, sp.submitted_at, \
                   btrim(regexp_replace(sp.text, '[[:space:]]+', ' ', 'g')) AS norm \
              FROM submitted_prompts sp \
             WHERE sp.matched_at IS NULL AND sp.dismissed_at IS NULL \
         ), per_prompt AS ( \
            SELECT DISTINCT ON (o.id) o.id, o.submitted_at, t.session_uuid, t.turn_id \
              FROM open o \
              JOIN claude_sessions cs ON cs.pty_session_id = o.pty_session_id \
              JOIN timeline_turns t ON t.session_uuid = cs.session_uuid \
             WHERE t.user_prompt_text IS NOT NULL \
               AND t.start_timestamp >= o.submitted_at - INTERVAL '10 seconds' \
               AND btrim(regexp_replace(t.user_prompt_text, '[[:space:]]+', ' ', 'g')) = o.norm \
               AND NOT EXISTS ( \
                   SELECT 1 FROM submitted_prompts m \
                    WHERE m.matched_session_uuid = t.session_uuid \
                      AND m.matched_turn_id = t.turn_id \
               ) \
             ORDER BY o.id, t.start_timestamp ASC \
         ), per_turn AS ( \
            SELECT DISTINCT ON (session_uuid, turn_id) id, session_uuid, turn_id \
              FROM per_prompt \
             ORDER BY session_uuid, turn_id, submitted_at ASC \
         ) \
         UPDATE submitted_prompts sp \
            SET matched_session_uuid = c.session_uuid, \
                matched_turn_id = c.turn_id, \
                matched_at = NOW() \
           FROM per_turn c \
          WHERE sp.id = c.id",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn prune(pool: &Pool) -> anyhow::Result<u64> {
    let result = sqlx::query(
        "DELETE FROM submitted_prompts \
          WHERE (matched_at IS NOT NULL AND matched_at < NOW() - make_interval(days => $1)) \
             OR (dismissed_at IS NOT NULL AND dismissed_at < NOW() - make_interval(days => $1))",
    )
    .bind(RETENTION_DAYS)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Control-process loop: reconcile every [`RECONCILE_INTERVAL`], prune
/// roughly once an hour. Errors are logged and the loop continues; a
/// transient database outage must not stop matching for good.
pub async fn run_reconciler(pool: Pool) {
    let prune_every = (3600 / RECONCILE_INTERVAL.as_secs().max(1)) as u32;
    let mut tick: u32 = 0;
    loop {
        if let Err(err) = reconcile(&pool).await {
            tracing::warn!(
                error = format!("{err:#}"),
                "submitted prompt reconcile failed"
            );
        }
        tick = tick.wrapping_add(1);
        if tick.is_multiple_of(prune_every) {
            if let Err(err) = prune(&pool).await {
                tracing::warn!(error = format!("{err:#}"), "submitted prompt prune failed");
            }
        }
        tokio::time::sleep(RECONCILE_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn correlation_counts_only_from_the_current_launch() {
        let session = Some(Uuid::new_v4());
        let started = Utc::now();
        let before = Some(started - Duration::seconds(30));
        let after = Some(started + Duration::seconds(30));

        assert!(!correlated_for_launch(None, after, Some(started)));
        assert!(!correlated_for_launch(session, None, Some(started)));
        assert!(!correlated_for_launch(session, before, Some(started)));
        assert!(correlated_for_launch(session, after, Some(started)));
        assert!(correlated_for_launch(session, Some(started), Some(started)));
        assert!(correlated_for_launch(session, before, None));
    }

    #[test]
    fn harness_reported_activity_counts_as_ready() {
        let started = Some(Utc::now());
        assert!(!harness_ready(None, None, started, None));
        assert!(!harness_ready(None, None, started, Some("launcher")));
        assert!(!harness_ready(None, None, started, Some("user")));
        assert!(harness_ready(None, None, started, Some("hook")));
        assert!(harness_ready(None, None, started, Some("agent")));
        assert!(harness_ready(None, None, started, Some("ingester")));
    }
}
