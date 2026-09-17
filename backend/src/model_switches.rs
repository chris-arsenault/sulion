//! Model switches observed in transcript sessions.
//!
//! Both harnesses can move a session onto a different model without the
//! user asking. Codex applies new thread settings between turns
//! (`thread_settings_applied`) and starts the next turn on the new model
//! with a `<model_switch>` developer note; Claude Code writes a `fallback`
//! content block mid-turn when the primary model's request is retried on
//! the fallback model. Neither records why. In the timeline the model is
//! only a metadata line, so a downgrade is easy to miss.
//!
//! The ingester calls [`observe_event`] for every stored record. It keeps
//! two baselines per session on `agent_session_metadata`: `observed_model`,
//! the model the last record ran on, and `confirmed_model`, the model the
//! session launched with or the user later accepted in the timeline's
//! switch dialog. A change from the observed model records a row in
//! `agent_model_switches`; a change away from the confirmed model is
//! `enforced`. The control process stops the turn running under an
//! enforced, unacknowledged switch ([`pending_enforcement`]) and the
//! timeline opens a dialog that [`acknowledge`] closes. Returning to the
//! confirmed model is recorded but never enforced, so restoring the model
//! by hand does not trip the guard.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::db::Pool;

/// Rows returned by [`list_for_pty`].
pub const RECENT_LIMIT: i64 = 50;

/// How often the control process looks for switches to enforce.
pub const ENFORCE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Placeholder model Claude Code stamps on synthetic (error) records.
const CLAUDE_SYNTHETIC_MODEL: &str = "<synthetic>";

/// A transcript record that names the model a session is running on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelObservation {
    pub source: &'static str,
    pub model: String,
    pub effort: Option<String>,
    pub turn_id: Option<String>,
    /// Record-local context worth keeping with a switch (the fallback
    /// block, request iterations, service tier).
    pub context: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ModelSwitch {
    pub id: Uuid,
    pub session_uuid: Uuid,
    pub agent: String,
    pub source: String,
    pub from_model: Option<String>,
    pub to_model: String,
    pub from_effort: Option<String>,
    pub to_effort: Option<String>,
    pub turn_id: Option<String>,
    pub turn_in_flight: bool,
    pub enforced: bool,
    pub context: Value,
    pub observed_at: DateTime<Utc>,
    pub detected_at: DateTime<Utc>,
    pub interrupted_at: Option<DateTime<Utc>>,
    pub interrupt_error: Option<String>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub adopted: Option<bool>,
}

/// The model a record reports, if it reports one.
pub fn observation_from_event(agent: &str, value: &Value) -> Option<ModelObservation> {
    match agent {
        "codex" => codex_observation(value),
        "claude-code" | "claude" => claude_observation(value),
        _ => None,
    }
}

fn codex_observation(value: &Value) -> Option<ModelObservation> {
    let kind = crate::ingest::canonical::codex_record_kind(value)?;
    let payload = value.get("payload")?;
    match kind {
        "turn_context" => Some(ModelObservation {
            source: "codex_turn_context",
            model: non_empty(payload.get("model"))?,
            effort: non_empty(payload.get("effort"))
                .or_else(|| non_empty(payload.get("reasoning_effort"))),
            turn_id: non_empty(payload.get("turn_id")),
            context: Map::new(),
        }),
        "event_msg"
            if payload.get("type").and_then(Value::as_str) == Some("thread_settings_applied") =>
        {
            let settings = payload.get("thread_settings")?;
            let mut context = Map::new();
            for key in ["service_tier", "model_provider_id"] {
                if let Some(text) = non_empty(settings.get(key)) {
                    context.insert(key.to_string(), Value::String(text));
                }
            }
            Some(ModelObservation {
                source: "codex_thread_settings",
                model: non_empty(settings.get("model"))?,
                effort: non_empty(settings.get("reasoning_effort")),
                turn_id: None,
                context,
            })
        }
        _ => None,
    }
}

fn claude_observation(value: &Value) -> Option<ModelObservation> {
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let message = value.get("message")?;
    let model = non_empty(message.get("model"))?;
    if model == CLAUDE_SYNTHETIC_MODEL {
        return None;
    }
    let fallback = message
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("type").and_then(Value::as_str) == Some("fallback"))
        });
    let mut context = Map::new();
    if let Some(fallback) = fallback {
        context.insert(
            "fallback".to_string(),
            json!({
                "from": fallback.get("from").and_then(|from| from.get("model")),
                "to": fallback.get("to").and_then(|to| to.get("model")),
            }),
        );
    }
    let usage = message.get("usage");
    if let Some(iterations) = usage
        .and_then(|usage| usage.get("iterations"))
        .and_then(Value::as_array)
    {
        context.insert(
            "iterations".to_string(),
            Value::Array(
                iterations
                    .iter()
                    .map(|iteration| {
                        json!({
                            "type": iteration.get("type"),
                            "model": iteration.get("model"),
                        })
                    })
                    .collect(),
            ),
        );
    }
    if let Some(tier) = usage.and_then(|usage| non_empty(usage.get("service_tier"))) {
        context.insert("service_tier".to_string(), Value::String(tier));
    }
    Some(ModelObservation {
        source: if fallback.is_some() {
            "claude_fallback"
        } else {
            "claude_message"
        },
        model,
        effort: non_empty(value.get("effort")),
        turn_id: None,
        context,
    })
}

fn non_empty(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Compare a stored record against the session's baselines and record a
/// switch when the model moved. Returns the new row's id, if one was
/// inserted. Called after the event row is committed; runs in its own
/// transaction so the baseline read and update are atomic across
/// concurrent ticks.
pub async fn observe_event(
    pool: &Pool,
    session_uuid: Uuid,
    agent: &str,
    value: &Value,
    byte_offset: i64,
    observed_at: DateTime<Utc>,
) -> anyhow::Result<Option<Uuid>> {
    let Some(observation) = observation_from_event(agent, value) else {
        return Ok(None);
    };
    let mut tx = pool.begin().await?;
    let baseline: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT observed_model, observed_effort, confirmed_model \
           FROM agent_session_metadata WHERE session_uuid = $1 FOR UPDATE",
    )
    .bind(session_uuid)
    .fetch_optional(&mut *tx)
    .await?;
    let (observed_model, observed_effort, confirmed_model) = baseline.unwrap_or((None, None, None));

    // First sighting: the launch model is what the user asked for.
    let Some(previous_model) = observed_model.filter(|model| !model.is_empty()) else {
        write_baseline(
            &mut tx,
            session_uuid,
            agent,
            &observation.model,
            observation.effort.as_deref(),
            &observation.model,
        )
        .await?;
        tx.commit().await?;
        return Ok(None);
    };
    if previous_model == observation.model {
        if observed_effort != observation.effort {
            write_baseline(
                &mut tx,
                session_uuid,
                agent,
                &observation.model,
                observation.effort.as_deref(),
                &previous_model,
            )
            .await?;
        }
        tx.commit().await?;
        return Ok(None);
    }

    let confirmed_model = confirmed_model.unwrap_or_else(|| previous_model.clone());
    let enforced = observation.model != confirmed_model;
    let (turn_in_flight, turn_id) =
        turn_state(&mut tx, session_uuid, &observation, byte_offset).await?;
    let mut context = observation.context.clone();
    if agent == "codex" {
        let rate_limits: Option<Value> = sqlx::query_scalar(
            "SELECT payload->'payload'->'rate_limits' \
               FROM events \
              WHERE session_uuid = $1 AND kind = 'token_count' AND byte_offset < $2 \
              ORDER BY byte_offset DESC \
              LIMIT 1",
        )
        .bind(session_uuid)
        .bind(byte_offset)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(rate_limits) = rate_limits.filter(|value| value.is_object()) {
            context.insert("rate_limits".to_string(), rate_limits);
        }
    }

    let id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO agent_model_switches \
             (session_uuid, agent, byte_offset, observed_at, source, from_model, to_model, \
              from_effort, to_effort, turn_id, turn_in_flight, enforced, context) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
         ON CONFLICT (session_uuid, byte_offset) DO NOTHING \
         RETURNING id",
    )
    .bind(session_uuid)
    .bind(agent)
    .bind(byte_offset)
    .bind(observed_at)
    .bind(observation.source)
    .bind(&previous_model)
    .bind(&observation.model)
    .bind(observed_effort.as_deref())
    .bind(observation.effort.as_deref())
    .bind(turn_id.as_deref())
    .bind(turn_in_flight)
    .bind(enforced)
    .bind(Value::Object(context))
    .fetch_optional(&mut *tx)
    .await?;
    write_baseline(
        &mut tx,
        session_uuid,
        agent,
        &observation.model,
        observation.effort.as_deref(),
        &confirmed_model,
    )
    .await?;
    tx.commit().await?;
    if id.is_some() {
        tracing::info!(
            session = %session_uuid,
            agent,
            from = %previous_model,
            to = %observation.model,
            enforced,
            turn_in_flight,
            "model switch observed",
        );
    }
    Ok(id)
}

/// Whether a turn was underway at `byte_offset`, and which one. Codex
/// brackets turns with `task_started` / `task_complete` / `turn_aborted`
/// records; a `turn_context` is itself the start of one. Claude assistant
/// records only exist inside a turn.
async fn turn_state(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session_uuid: Uuid,
    observation: &ModelObservation,
    byte_offset: i64,
) -> anyhow::Result<(bool, Option<String>)> {
    if observation.source != "codex_thread_settings" {
        return Ok((true, observation.turn_id.clone()));
    }
    let last: Option<(String, Value)> = sqlx::query_as(
        "SELECT kind, payload \
           FROM events \
          WHERE session_uuid = $1 AND byte_offset < $2 \
            AND kind IN ('task_started', 'task_complete', 'turn_started', 'turn_complete', 'turn_aborted') \
          ORDER BY byte_offset DESC \
          LIMIT 1",
    )
    .bind(session_uuid)
    .bind(byte_offset)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(match last {
        Some((kind, payload)) if matches!(kind.as_str(), "task_started" | "turn_started") => {
            let turn_id = payload
                .get("payload")
                .and_then(|payload| payload.get("turn_id"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            (true, turn_id)
        }
        _ => (false, None),
    })
}

async fn write_baseline(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session_uuid: Uuid,
    agent: &str,
    observed_model: &str,
    observed_effort: Option<&str>,
    confirmed_model: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO agent_session_metadata \
             (session_uuid, agent, observed_model, observed_effort, confirmed_model, updated_at) \
         VALUES ($1, $2, $3, $4, $5, NOW()) \
         ON CONFLICT (session_uuid) DO UPDATE SET \
             observed_model = EXCLUDED.observed_model, \
             observed_effort = EXCLUDED.observed_effort, \
             confirmed_model = COALESCE(agent_session_metadata.confirmed_model, EXCLUDED.confirmed_model), \
             updated_at = NOW()",
    )
    .bind(session_uuid)
    .bind(agent)
    .bind(observed_model)
    .bind(observed_effort)
    .bind(confirmed_model)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

const SWITCH_COLUMNS: &str = "s.id, s.session_uuid, s.agent, s.source, s.from_model, s.to_model, \
     s.from_effort, s.to_effort, s.turn_id, s.turn_in_flight, s.enforced, s.context, \
     s.observed_at, s.detected_at, s.interrupted_at, s.interrupt_error, s.acknowledged_at, \
     s.adopted";

/// Recent switches in the PTY's current transcript session, newest first.
pub async fn list_for_pty(pool: &Pool, pty_session_id: Uuid) -> anyhow::Result<Vec<ModelSwitch>> {
    let rows = sqlx::query_as::<_, ModelSwitch>(&format!(
        "SELECT {SWITCH_COLUMNS} \
           FROM agent_model_switches s \
           JOIN pty_sessions ps ON ps.current_session_uuid = s.session_uuid \
          WHERE ps.id = $1 \
          ORDER BY s.observed_at DESC, s.byte_offset DESC \
          LIMIT $2"
    ))
    .bind(pty_session_id)
    .bind(RECENT_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Close a switch from the timeline dialog. `adopt` accepts the new model
/// as the session's confirmed model, so later records on it are no longer
/// a departure; without it the confirmed model stays what it was, for a
/// user who intends to switch back by hand. Returns false when the switch
/// is not an open one on this PTY's current session.
pub async fn acknowledge(
    pool: &Pool,
    pty_session_id: Uuid,
    switch_id: Uuid,
    adopt: bool,
) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let closed: Option<(Uuid, String)> = sqlx::query_as(
        "UPDATE agent_model_switches s \
            SET acknowledged_at = NOW(), adopted = $3 \
           FROM pty_sessions ps \
          WHERE s.id = $2 AND ps.id = $1 \
            AND ps.current_session_uuid = s.session_uuid \
            AND s.acknowledged_at IS NULL \
         RETURNING s.session_uuid, s.to_model",
    )
    .bind(pty_session_id)
    .bind(switch_id)
    .bind(adopt)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((session_uuid, to_model)) = closed else {
        return Ok(false);
    };
    if adopt {
        sqlx::query(
            "UPDATE agent_session_metadata SET confirmed_model = $2, updated_at = NOW() \
              WHERE session_uuid = $1",
        )
        .bind(session_uuid)
        .bind(&to_model)
        .execute(&mut *tx)
        .await?;
        // Any other open switch onto the same model is answered by the
        // same decision.
        sqlx::query(
            "UPDATE agent_model_switches \
                SET acknowledged_at = NOW(), adopted = TRUE \
              WHERE session_uuid = $1 AND to_model = $2 AND acknowledged_at IS NULL",
        )
        .bind(session_uuid)
        .bind(&to_model)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// Enforced, unacknowledged switches whose PTY currently has a turn
/// running: each is a turn on a model the user has not accepted. A row is
/// returned until its interrupt lands, so a node that was unreachable is
/// retried on the next pass.
pub async fn pending_enforcement(pool: &Pool) -> anyhow::Result<Vec<(Uuid, Uuid)>> {
    let rows = sqlx::query_as(
        "SELECT s.id, ps.id \
           FROM agent_model_switches s \
           JOIN pty_sessions ps ON ps.current_session_uuid = s.session_uuid \
           JOIN session_activity_state sas ON sas.pty_session_id = ps.id \
          WHERE s.enforced AND s.acknowledged_at IS NULL AND s.interrupted_at IS NULL \
            AND ps.state = 'live' AND ps.agent_runtime_state = 'running' \
            AND sas.state = 'working' \
          ORDER BY s.observed_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn mark_interrupted(pool: &Pool, switch_id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE agent_model_switches \
            SET interrupted_at = NOW(), interrupt_error = NULL \
          WHERE id = $1",
    )
    .bind(switch_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_interrupt_error(pool: &Pool, switch_id: Uuid, error: &str) -> anyhow::Result<()> {
    let error: String = error.chars().take(500).collect();
    sqlx::query("UPDATE agent_model_switches SET interrupt_error = $2 WHERE id = $1")
        .bind(switch_id)
        .bind(error)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_thread_settings_name_the_model_and_effort() {
        let value = json!({
            "type": "event_msg",
            "payload": {
                "type": "thread_settings_applied",
                "thread_id": "t",
                "thread_settings": {
                    "model": "gpt-5.6-luna",
                    "model_provider_id": "openai",
                    "service_tier": "default",
                    "reasoning_effort": "medium"
                }
            }
        });
        let observation = observation_from_event("codex", &value).expect("observation");
        assert_eq!(observation.source, "codex_thread_settings");
        assert_eq!(observation.model, "gpt-5.6-luna");
        assert_eq!(observation.effort.as_deref(), Some("medium"));
        assert_eq!(observation.turn_id, None);
        assert_eq!(observation.context["service_tier"], "default");
    }

    #[test]
    fn codex_turn_context_names_the_turn() {
        let value = json!({
            "type": "turn_context",
            "payload": { "turn_id": "turn-1", "model": "gpt-6-astra", "effort": "high" }
        });
        let observation = observation_from_event("codex", &value).expect("observation");
        assert_eq!(observation.source, "codex_turn_context");
        assert_eq!(observation.model, "gpt-6-astra");
        assert_eq!(observation.effort.as_deref(), Some("high"));
        assert_eq!(observation.turn_id.as_deref(), Some("turn-1"));
    }

    #[test]
    fn codex_other_records_carry_no_model() {
        let value = json!({
            "type": "event_msg",
            "payload": { "type": "task_started", "turn_id": "turn-1" }
        });
        assert!(observation_from_event("codex", &value).is_none());
        let value = json!({
            "type": "event_msg",
            "payload": { "type": "collab_agent_spawn_end", "model": "gpt-5.6-luna" }
        });
        assert!(observation_from_event("codex", &value).is_none());
    }

    #[test]
    fn claude_fallback_block_is_a_switch_with_context() {
        let value = json!({
            "type": "assistant",
            "effort": "high",
            "message": {
                "model": "claude-opus-4-8",
                "role": "assistant",
                "content": [
                    { "type": "fallback", "from": { "model": "claude-fable-5" }, "to": { "model": "claude-opus-4-8" } }
                ],
                "usage": {
                    "service_tier": "standard",
                    "iterations": [
                        { "type": "message", "model": "claude-fable-5", "output_tokens": 760 },
                        { "type": "fallback_message", "model": "claude-opus-4-8", "output_tokens": 244 }
                    ]
                }
            }
        });
        let observation = observation_from_event("claude-code", &value).expect("observation");
        assert_eq!(observation.source, "claude_fallback");
        assert_eq!(observation.model, "claude-opus-4-8");
        assert_eq!(observation.effort.as_deref(), Some("high"));
        assert_eq!(observation.context["fallback"]["from"], "claude-fable-5");
        assert_eq!(observation.context["fallback"]["to"], "claude-opus-4-8");
        assert_eq!(
            observation.context["iterations"][1]["type"],
            "fallback_message"
        );
        assert_eq!(observation.context["service_tier"], "standard");
    }

    #[test]
    fn claude_ordinary_assistant_record_names_its_model() {
        let value = json!({
            "type": "assistant",
            "message": { "model": "claude-fable-5-1", "content": [{ "type": "text", "text": "hi" }] }
        });
        let observation = observation_from_event("claude-code", &value).expect("observation");
        assert_eq!(observation.source, "claude_message");
        assert_eq!(observation.model, "claude-fable-5-1");
        assert!(observation.context.is_empty());
    }

    #[test]
    fn claude_synthetic_and_non_assistant_records_are_ignored() {
        let synthetic = json!({
            "type": "assistant",
            "message": { "model": "<synthetic>", "content": [{ "type": "text", "text": "API error" }] }
        });
        assert!(observation_from_event("claude-code", &synthetic).is_none());
        let user =
            json!({ "type": "user", "message": { "model": "claude-opus-5", "content": "hi" } });
        assert!(observation_from_event("claude-code", &user).is_none());
    }
}
