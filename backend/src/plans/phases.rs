//! Phase-level operations on a published plan: adding, editing, reordering,
//! and the bulk skip that an explicit `skip_remaining` close performs.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::model::{NewPhase, PlanActor, PlanPhaseView, PlanView, UpdatePhaseInput};
use super::{
    bump_revision, clean_note, ensure_plan_open, get, insert_event, limited_text, required_text,
    validate_actor, MAX_DESCRIPTION_CHARS, MAX_TITLE_CHARS,
};
use crate::db::Pool;

pub async fn add_phase(
    pool: &Pool,
    plan_id: Uuid,
    phase: NewPhase,
    actor: &PlanActor,
) -> anyhow::Result<PlanView> {
    validate_actor(actor)?;
    let title = required_text(&phase.title, "phase title", MAX_TITLE_CHARS)?;
    let description = limited_text(
        &phase.description,
        "phase description",
        MAX_DESCRIPTION_CHARS,
    )?;
    let status = phase.status.as_deref().unwrap_or("pending");
    validate_phase_status(status)?;
    let size = validate_phase_size(phase.size.as_deref())?;
    let mut tx = pool.begin().await?;
    ensure_plan_open(&mut tx, plan_id).await?;
    let position: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position), 0)::INT + 1 FROM plan_phases WHERE plan_id = $1",
    )
    .bind(plan_id)
    .fetch_one(&mut *tx)
    .await?;
    let phase_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO plan_phases \
             (id, plan_id, position, title, description, status, size, started_at, completed_at, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, \
                 CASE WHEN $6 = 'in_progress' THEN NOW() ELSE NULL END, \
                 CASE WHEN $6 IN ('completed', 'skipped') THEN NOW() ELSE NULL END, NOW(), NOW())",
    )
    .bind(phase_id)
    .bind(plan_id)
    .bind(position)
    .bind(title)
    .bind(description)
    .bind(status)
    .bind(size)
    .execute(&mut *tx)
    .await?;
    bump_revision(&mut tx, plan_id).await?;
    insert_event(
        &mut tx,
        plan_id,
        Some(phase_id),
        "phase_added",
        actor,
        None,
        Some(status),
        None,
    )
    .await?;
    tx.commit().await?;
    get(pool, plan_id).await
}

pub async fn update_phase(
    pool: &Pool,
    plan_id: Uuid,
    phase_id: Uuid,
    input: UpdatePhaseInput,
    actor: &PlanActor,
) -> anyhow::Result<PlanView> {
    validate_actor(actor)?;
    let mut tx = pool.begin().await?;
    ensure_plan_open(&mut tx, plan_id).await?;
    let current: PlanPhaseView = sqlx::query_as(
        "SELECT id, plan_id, position, title, description, status, status_note, size, \
                started_at, completed_at, created_at, updated_at \
           FROM plan_phases WHERE id = $1 AND plan_id = $2 FOR UPDATE",
    )
    .bind(phase_id)
    .bind(plan_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("phase not found: {phase_id}"))?;
    let title = match input.title.as_deref() {
        Some(value) => required_text(value, "phase title", MAX_TITLE_CHARS)?,
        None => current.title,
    };
    let description = match input.description.as_deref() {
        Some(value) => limited_text(value, "phase description", MAX_DESCRIPTION_CHARS)?,
        None => current.description,
    };
    let status = input.status.as_deref().unwrap_or(&current.status);
    validate_phase_status(status)?;
    let status_note = match input.status_note.as_deref() {
        Some(value) => clean_note(Some(value))?,
        None => current.status_note,
    };
    let size = match input.size.as_deref() {
        Some(value) => validate_phase_size(Some(value))?,
        None => current.size,
    };
    if let Some(position) = input.position {
        move_phase(&mut tx, plan_id, phase_id, current.position, position).await?;
    }
    sqlx::query(
        "UPDATE plan_phases \
            SET title = $3, description = $4, status = $5, status_note = $6, size = $7, \
                started_at = CASE \
                    WHEN $5 IN ('in_progress', 'blocked') THEN COALESCE(started_at, NOW()) \
                    WHEN $5 = 'pending' THEN NULL ELSE started_at END, \
                completed_at = CASE WHEN $5 IN ('completed', 'skipped') THEN COALESCE(completed_at, NOW()) ELSE NULL END, \
                updated_at = NOW() \
          WHERE id = $1 AND plan_id = $2",
    )
    .bind(phase_id)
    .bind(plan_id)
    .bind(title)
    .bind(description)
    .bind(status)
    .bind(status_note.as_deref())
    .bind(size.as_deref())
    .execute(&mut *tx)
    .await?;
    bump_revision(&mut tx, plan_id).await?;
    insert_event(
        &mut tx,
        plan_id,
        Some(phase_id),
        if status == current.status {
            "phase_updated"
        } else {
            "phase_status_changed"
        },
        actor,
        Some(&current.status),
        Some(status),
        status_note.as_deref(),
    )
    .await?;
    tx.commit().await?;
    get(pool, plan_id).await
}

pub async fn resolve_phase_id(pool: &Pool, plan_id: Uuid, reference: &str) -> anyhow::Result<Uuid> {
    if let Ok(id) = Uuid::parse_str(reference) {
        let found: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM plan_phases WHERE plan_id = $1 AND id = $2")
                .bind(plan_id)
                .bind(id)
                .fetch_optional(pool)
                .await?;
        return found.ok_or_else(|| anyhow::anyhow!("phase not found: {reference}"));
    }
    let position: i32 = reference
        .parse()
        .map_err(|_| anyhow::anyhow!("phase must be a UUID or 1-based position"))?;
    sqlx::query_scalar("SELECT id FROM plan_phases WHERE plan_id = $1 AND position = $2")
        .bind(plan_id)
        .bind(position)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| anyhow::anyhow!("phase not found at position {position}"))
}

async fn move_phase(
    tx: &mut Transaction<'_, Postgres>,
    plan_id: Uuid,
    phase_id: Uuid,
    old_position: i32,
    requested_position: i32,
) -> anyhow::Result<()> {
    let count: i32 = sqlx::query_scalar("SELECT COUNT(*)::INT FROM plan_phases WHERE plan_id = $1")
        .bind(plan_id)
        .fetch_one(&mut **tx)
        .await?;
    let new_position = requested_position.clamp(1, count);
    if new_position == old_position {
        return Ok(());
    }
    sqlx::query("SET CONSTRAINTS plan_phases_plan_position_key DEFERRED")
        .execute(&mut **tx)
        .await?;
    if new_position < old_position {
        sqlx::query(
            "UPDATE plan_phases SET position = position + 1, updated_at = NOW() \
              WHERE plan_id = $1 AND position >= $2 AND position < $3 AND id <> $4",
        )
        .bind(plan_id)
        .bind(new_position)
        .bind(old_position)
        .bind(phase_id)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query(
            "UPDATE plan_phases SET position = position - 1, updated_at = NOW() \
              WHERE plan_id = $1 AND position > $2 AND position <= $3 AND id <> $4",
        )
        .bind(plan_id)
        .bind(old_position)
        .bind(new_position)
        .bind(phase_id)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query("UPDATE plan_phases SET position = $3 WHERE plan_id = $1 AND id = $2")
        .bind(plan_id)
        .bind(phase_id)
        .bind(new_position)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(super) async fn skip_remaining_phases(
    tx: &mut Transaction<'_, Postgres>,
    plan_id: Uuid,
    actor: &PlanActor,
    note: Option<&str>,
) -> anyhow::Result<()> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, status FROM plan_phases \
          WHERE plan_id = $1 AND status NOT IN ('completed', 'skipped') \
          ORDER BY position FOR UPDATE",
    )
    .bind(plan_id)
    .fetch_all(&mut **tx)
    .await?;
    for (phase_id, from_status) in rows {
        sqlx::query(
            "UPDATE plan_phases \
                SET status = 'skipped', status_note = COALESCE($3, status_note), \
                    completed_at = NOW(), updated_at = NOW() \
              WHERE plan_id = $1 AND id = $2",
        )
        .bind(plan_id)
        .bind(phase_id)
        .bind(note)
        .execute(&mut **tx)
        .await?;
        insert_event(
            tx,
            plan_id,
            Some(phase_id),
            "phase_status_changed",
            actor,
            Some(&from_status),
            Some("skipped"),
            note,
        )
        .await?;
    }
    Ok(())
}

pub(super) fn initial_phase_status(
    requested: Option<&str>,
    index: usize,
    all_pending: bool,
) -> anyhow::Result<String> {
    let status = requested.unwrap_or({
        if index == 0 && !all_pending {
            "in_progress"
        } else {
            "pending"
        }
    });
    validate_phase_status(status)?;
    Ok(status.to_string())
}

pub(super) fn validate_phase_status(status: &str) -> anyhow::Result<()> {
    if matches!(
        status,
        "pending" | "in_progress" | "blocked" | "completed" | "skipped"
    ) {
        Ok(())
    } else {
        anyhow::bail!("invalid phase status: {status}")
    }
}

/// Normalize an optional phase size. Case-insensitive; empty clears it.
pub(super) fn validate_phase_size(size: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(raw) = size else { return Ok(None) };
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(None);
    }
    if matches!(normalized.as_str(), "s" | "m" | "l") {
        Ok(Some(normalized))
    } else {
        anyhow::bail!("invalid phase size: {raw} (expected s, m, or l)")
    }
}
