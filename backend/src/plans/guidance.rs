use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{limited_text, required_text, PlanActor, PlanGuidance, UpdatePlanInput};

impl PlanGuidance {
    pub(super) fn with_update(&self, input: &UpdatePlanInput) -> anyhow::Result<Self> {
        Self {
            outcome: input.outcome.as_ref().unwrap_or(&self.outcome).clone(),
            principles: input
                .principles
                .as_ref()
                .unwrap_or(&self.principles)
                .clone(),
            assumptions: input
                .assumptions
                .as_ref()
                .unwrap_or(&self.assumptions)
                .clone(),
        }
        .validated()
    }

    pub(super) fn validated(&self) -> anyhow::Result<Self> {
        Ok(Self {
            outcome: limited_text(&self.outcome, "outcome", 1_000)?,
            principles: validate_items(&self.principles, "principle")?,
            assumptions: validate_items(&self.assumptions, "assumption")?,
        })
    }
}

fn validate_items(items: &[String], label: &str) -> anyhow::Result<Vec<String>> {
    if items.len() > 10 {
        anyhow::bail!("guidance allows at most 10 {label} entries");
    }
    items
        .iter()
        .map(|item| required_text(item, label, 500))
        .collect()
}

pub(super) async fn record_change(
    tx: &mut Transaction<'_, Postgres>,
    plan_id: Uuid,
    actor: &PlanActor,
    before: Option<&PlanGuidance>,
    after: &PlanGuidance,
    note: Option<&str>,
) -> anyhow::Result<()> {
    if before == Some(after) || (before.is_none() && *after == PlanGuidance::default()) {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO plan_events \
         (plan_id, event_type, actor_kind, pty_session_id, agent_session_uuid, \
          note, guidance_before, guidance_after) \
         VALUES ($1, 'guidance_changed', $2, $3, $4, $5, $6, $7)",
    )
    .bind(plan_id)
    .bind(&actor.kind)
    .bind(actor.pty_session_id)
    .bind(actor.agent_session_uuid)
    .bind(note)
    .bind(before.map(sqlx::types::Json))
    .bind(sqlx::types::Json(after))
    .execute(&mut **tx)
    .await?;
    Ok(())
}
