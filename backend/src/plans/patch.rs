use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Value};
use sqlx::{types::Json, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::authoring::{
    AuthoredPlan, AuthoredPlanView, DirectEdit, EntryContent, EntryKind, PhaseEvidence,
    PhaseFields, PlanEdit, PlanEntry, PlanFields, PlanPatch,
};
use super::{
    ancestors, clean_note, get, guidance, insert_event, limited_text, required_text,
    validate_actor, PhaseReference, PlanActor, PlanGuidance, PlanRow, PlanView,
    MAX_DESCRIPTION_CHARS, MAX_SUMMARY_CHARS, MAX_TITLE_CHARS, PLAN_COLUMNS,
};
use crate::db::Pool;

#[derive(Debug, thiserror::Error)]
#[error("plan authoring revision conflict: expected {expected}, current {current}; reread the plan and its history before retrying")]
pub struct AuthoringConflict {
    pub expected: i64,
    pub current: i64,
}

pub(super) async fn read_authoring(
    connection: &mut PgConnection,
    id: Uuid,
    guidance: &PlanGuidance,
) -> anyhow::Result<(AuthoredPlanView, bool)> {
    let (revision, stored): (i64, Option<Json<AuthoredPlan>>) =
        sqlx::query_as("SELECT authoring_revision, authoring FROM plans WHERE id = $1")
            .bind(id)
            .fetch_one(connection)
            .await?;
    let managed = stored.is_some();
    let document = stored
        .map(|value| value.0)
        .unwrap_or_else(|| AuthoredPlan::from_guidance(id, guidance));
    Ok((
        AuthoredPlanView {
            revision,
            document,
            needs_review: BTreeMap::new(),
        },
        managed,
    ))
}

pub(super) fn check_revision(expected: i64, current: i64) -> anyhow::Result<()> {
    if expected != current {
        return Err(AuthoringConflict { expected, current }.into());
    }
    Ok(())
}

pub async fn apply_direct(
    pool: &Pool,
    id: Uuid,
    input: DirectEdit<impl Into<PhaseReference> + Send>,
    actor: &PlanActor,
    client: &str,
    subject: Option<&str>,
    expected_repo: Option<&str>,
) -> anyhow::Result<PlanView> {
    apply(
        pool,
        id,
        PlanPatch {
            revision: input.revision,
            reason: input.reason,
            source: None,
            operations: vec![input.operation],
        },
        actor,
        client,
        subject,
        expected_repo,
    )
    .await
}

pub async fn apply(
    pool: &Pool,
    id: Uuid,
    input: PlanPatch<impl Into<Option<i64>> + Send, impl Into<PhaseReference> + Send>,
    actor: &PlanActor,
    client: &str,
    subject: Option<&str>,
    expected_repo: Option<&str>,
) -> anyhow::Result<PlanView> {
    validate_actor(actor)?;
    if input.operations.is_empty() {
        anyhow::bail!("plan patch needs at least one operation");
    }
    let reason = clean_note(input.reason.as_deref())?;
    let source = clean_note(input.source.as_deref())?;
    let mut tx = pool.begin().await?;
    let mut row: PlanRow = sqlx::query_as(&format!(
        "SELECT {PLAN_COLUMNS} FROM plans WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("plan not found: {id}"))?;
    if expected_repo.is_some_and(|repo| repo != row.repo_name) {
        anyhow::bail!("plan not found: {id}");
    }
    if matches!(row.status.as_str(), "completed" | "canceled") {
        anyhow::bail!("plan is closed with status {}", row.status);
    }
    let (authored, _) = read_authoring(&mut tx, id, &row.guidance).await?;
    if let Some(expected) = input.revision.into() {
        check_revision(expected, authored.revision)?;
    }
    let mut document = authored.document;
    let old_guidance = row.guidance.clone();
    let mut changes = Vec::with_capacity(input.operations.len());
    let mut entry_successors = Vec::new();
    let mut phase_successors = Vec::new();
    for operation in input.operations {
        let operation = operation.resolve_phases(&mut tx, id).await?;
        match &operation {
            PlanEdit::RetireEntry { successors, .. } => {
                entry_successors.extend_from_slice(successors)
            }
            PlanEdit::RetirePhase { successors, .. } => {
                phase_successors.extend_from_slice(successors)
            }
            _ => (),
        }
        let change = apply_edit(
            &mut tx,
            &mut row,
            &mut document,
            operation,
            actor,
            authored.revision,
        )
        .await?;
        changes.push(change);
    }
    row.guidance = document.guidance(row.guidance.outcome).validated()?;
    validate_document(&mut tx, &row, &document).await?;
    check_current_successors(&mut tx, id, &document, &entry_successors, &phase_successors).await?;
    sqlx::query(
        "UPDATE plans SET title = $2, summary = $3, outcome = $4, principles = $5, assumptions = $6, \
         boundaries = $7, authoring = $8, authoring_revision = authoring_revision + 1, \
         revision = revision + 1, updated_at = NOW() WHERE id = $1",
    )
    .bind(id)
    .bind(&row.title)
    .bind(&row.summary)
    .bind(&row.guidance.outcome)
    .bind(&row.guidance.principles)
    .bind(&row.guidance.assumptions)
    .bind(Json(&row.guidance.boundaries))
    .bind(Json(&document))
    .execute(&mut *tx)
    .await?;
    guidance::record_change(
        &mut tx,
        id,
        actor,
        Some(&old_guidance),
        &row.guidance,
        reason.as_deref(),
    )
    .await?;
    record_change(
        &mut tx,
        id,
        authored.revision + 1,
        actor,
        client,
        reason.as_deref(),
        json!({"source": source, "subject": subject, "changes": changes}),
    )
    .await?;
    tx.commit().await?;
    get(pool, id).await
}

async fn check_current_successors(
    tx: &mut Transaction<'_, Postgres>,
    plan_id: Uuid,
    document: &AuthoredPlan,
    entries: &[Uuid],
    phases: &[Uuid],
) -> anyhow::Result<()> {
    for id in entries {
        if !document
            .entries
            .iter()
            .any(|entry| entry.id == *id && !entry.retired)
        {
            anyhow::bail!("entry successor must be current: {id}");
        }
    }
    if !phases.is_empty() {
        let retired: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM plan_phases WHERE plan_id = $1 AND id = ANY($2) AND retired_at IS NOT NULL LIMIT 1",
        )
        .bind(plan_id)
        .bind(phases)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(id) = retired {
            anyhow::bail!("phase successor must be current: {id}");
        }
    }
    Ok(())
}

pub(super) async fn record_change(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    revision: i64,
    actor: &PlanActor,
    client: &str,
    reason: Option<&str>,
    mut payload: Value,
) -> anyhow::Result<()> {
    payload["client"] = json!(client);
    sqlx::query(
        "INSERT INTO plan_events (plan_id, event_type, actor_kind, pty_session_id, agent_session_uuid, \
         note, authoring_revision, authoring_change) VALUES ($1, 'authoring_changed', $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(&actor.kind)
    .bind(actor.pty_session_id)
    .bind(actor.agent_session_uuid)
    .bind(reason)
    .bind(revision)
    .bind(Json(payload))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn validate_document(
    tx: &mut Transaction<'_, Postgres>,
    row: &PlanRow,
    document: &AuthoredPlan,
) -> anyhow::Result<()> {
    let mut inherited = HashSet::new();
    if row.parent_plan_id.is_some() {
        for ancestor in ancestors(tx, row.id).await? {
            let (view, _) = read_authoring(tx, ancestor.id, &ancestor.guidance).await?;
            inherited.extend(view.document.entries.into_iter().map(|entry| entry.id));
        }
    }
    let phases: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM plan_phases WHERE plan_id = $1")
        .bind(row.id)
        .fetch_all(&mut **tx)
        .await?;
    document.validate_links(&inherited, &phases.into_iter().collect())
}

async fn apply_edit(
    tx: &mut Transaction<'_, Postgres>,
    row: &mut PlanRow,
    document: &mut AuthoredPlan,
    edit: PlanEdit,
    actor: &PlanActor,
    revision: i64,
) -> anyhow::Result<Value> {
    let change = match edit {
        PlanEdit::EditPlan(PlanFields {
            title,
            summary,
            outcome,
            technical_details,
        }) => edit_plan_details(row, document, title, summary, outcome, technical_details)?,
        PlanEdit::AddEntry { entry } => put_entry(document, Uuid::new_v4(), entry)?,
        PlanEdit::EditEntry { id, fields } => {
            let updated = fields.apply(&document.entry_mut(id)?.content)?;
            put_entry(document, id, updated)?
        }
        PlanEdit::PutEntry { id, entry } => put_entry(document, id, entry)?,
        PlanEdit::MoveEntry { id, after } => move_entry(document, id, after)?,
        PlanEdit::RetireEntry { id, successors } => {
            let entry = document.entry_mut(id)?;
            let before = json!(entry);
            entry.retired = true;
            entry.successors = successors;
            json!({"op": "retire_entry", "id": id, "before": before, "after": entry})
        }
        PlanEdit::EditPhase(PhaseFields {
            id,
            title,
            description,
            implementation,
            acceptance,
            basis,
            depends_on,
            follows_up,
            size,
        }) => {
            let (old_title, old_description, old_size): (String, String, Option<String>) = sqlx::query_as(
                "SELECT title, description, size FROM plan_phases WHERE id = $1 AND plan_id = $2 AND retired_at IS NULL",
            ).bind(id).bind(row.id).fetch_optional(&mut **tx).await?
                .ok_or_else(|| anyhow::anyhow!("phase not found: {id}"))?;
            let details = document.phases.entry(id).or_default();
            let before = json!({"title": old_title, "description": old_description, "size": old_size, "details": details});
            let title = title
                .as_deref()
                .map(|value| required_text(value, "phase title", MAX_TITLE_CHARS))
                .transpose()?
                .unwrap_or(old_title);
            let description = description
                .as_deref()
                .map(|value| limited_text(value, "phase description", MAX_DESCRIPTION_CHARS))
                .transpose()?
                .unwrap_or(old_description);
            let size = if size.is_some() {
                super::validate_phase_size(size.as_deref())?
            } else {
                old_size
            };
            if let Some(implementation) = implementation {
                details.implementation = implementation;
            }
            if let Some(acceptance) = acceptance {
                details.acceptance = acceptance;
            }
            if let Some(basis) = basis {
                details.basis = basis;
            }
            if let Some(depends_on) = depends_on {
                details.depends_on = depends_on;
            }
            if let Some(follows_up) = follows_up {
                details.follows_up = follows_up;
            }
            sqlx::query("UPDATE plan_phases SET title = $3, description = $4, size = $5, updated_at = NOW() WHERE id = $1 AND plan_id = $2")
                .bind(id).bind(row.id).bind(&title).bind(&description).bind(&size).execute(&mut **tx).await?;
            json!({"op": "edit_phase", "id": id, "before": before, "after": {"title": title, "description": description, "size": size, "details": details}})
        }
        PlanEdit::AddPhase {
            id,
            title,
            description,
            size,
        } => {
            let title = required_text(&title, "phase title", MAX_TITLE_CHARS)?;
            let description =
                limited_text(&description, "phase description", MAX_DESCRIPTION_CHARS)?;
            let size = super::validate_phase_size(size.as_deref())?;
            sqlx::query("INSERT INTO plan_phases (id, plan_id, position, title, description, status, size) \
                         SELECT $1, $2, COALESCE(MAX(position), 0) + 1, $3, $4, 'pending', $5 FROM plan_phases WHERE plan_id = $2")
                .bind(id).bind(row.id).bind(&title).bind(&description).bind(&size).execute(&mut **tx).await?;
            insert_event(
                tx,
                row.id,
                Some(id),
                "phase_added",
                actor,
                None,
                Some("pending"),
                None,
            )
            .await?;
            json!({"op": "add_phase", "id": id, "after": {"title": title, "description": description, "size": size}})
        }
        PlanEdit::MovePhase { id, after } => move_phase(tx, row.id, id, after).await?,
        PlanEdit::RetirePhase { id, successors } => {
            retire_phase(tx, row.id, id, successors, document, actor).await?
        }
        PlanEdit::AttachDiagram {
            workspace_id,
            phase_links,
        } => {
            super::diagrams::validate_attachment(
                tx,
                &row.repo_name,
                row.id,
                workspace_id,
                &phase_links,
            )
            .await?;
            let before = document.diagrams.insert(workspace_id, phase_links);
            json!({"op": "attach_diagram", "id": workspace_id, "before": before,
                "after": document.diagrams[&workspace_id]})
        }
        PlanEdit::DetachDiagram { workspace_id } => {
            let before = document.diagrams.remove(&workspace_id);
            json!({"op": "detach_diagram", "id": workspace_id, "before": before})
        }
        PlanEdit::RecordEvidence {
            id,
            observation,
            reference,
            revision: observed,
            ancestors,
        } => {
            if !(0..=revision).contains(&observed) {
                anyhow::bail!("unknown authored revision for evidence: {observed}");
            }
            validate_evidence_ancestors(tx, row.id, &ancestors).await?;
            let evidence = PhaseEvidence {
                observation: required_text(&observation, "evidence observation", 20_000)?,
                reference,
                revision: observed,
                ancestors,
            };
            document
                .phases
                .entry(id)
                .or_default()
                .evidence
                .push(evidence.clone());
            json!({"op": "record_evidence", "id": id, "after": evidence})
        }
    };
    Ok(change)
}

fn edit_plan_details(
    row: &mut PlanRow,
    document: &mut AuthoredPlan,
    title: Option<String>,
    summary: Option<String>,
    outcome: Option<String>,
    technical_details: Option<String>,
) -> anyhow::Result<Value> {
    let before = json!({"title": row.title, "summary": row.summary, "outcome": row.guidance.outcome, "technical_details": document.technical_details});
    if let Some(title) = title {
        row.title = required_text(&title, "plan title", MAX_TITLE_CHARS)?;
    }
    if let Some(summary) = summary {
        row.summary = limited_text(&summary, "plan summary", MAX_SUMMARY_CHARS)?;
    }
    if let Some(outcome) = outcome {
        row.guidance.outcome = outcome;
    }
    if let Some(details) = technical_details {
        document.technical_details = details;
    }
    Ok(
        json!({"op": "edit_plan", "before": before, "after": {"title": row.title, "summary": row.summary, "outcome": row.guidance.outcome, "technical_details": document.technical_details}}),
    )
}

fn put_entry(
    document: &mut AuthoredPlan,
    id: Uuid,
    mut content: EntryContent,
) -> anyhow::Result<Value> {
    let limit = match content.kind {
        EntryKind::Principle | EntryKind::Assumption | EntryKind::Boundary { .. } => 500,
        _ => 20_000,
    };
    content.text = required_text(&content.text, "entry text", limit)?;
    if let Some(entry) = document.entries.iter_mut().find(|entry| entry.id == id) {
        if entry.retired
            || std::mem::discriminant(&entry.content.kind) != std::mem::discriminant(&content.kind)
        {
            anyhow::bail!("a retired entry or a change of entry kind requires a new ID: {id}");
        }
        let previous = std::mem::replace(&mut entry.content, content);
        Ok(json!({"op": "put_entry", "id": id, "before": previous, "after": entry.content}))
    } else {
        document.entries.push(PlanEntry {
            id,
            content,
            retired: false,
            successors: Vec::new(),
        });
        Ok(json!({"op": "put_entry", "id": id, "after": document.entries.last()}))
    }
}

fn move_entry(document: &mut AuthoredPlan, id: Uuid, after: Option<Uuid>) -> anyhow::Result<Value> {
    let from = document
        .entries
        .iter()
        .position(|item| item.id == id && !item.retired)
        .ok_or_else(|| anyhow::anyhow!("active entry not found: {id}"))?;
    let entry = document.entries.remove(from);
    let to = match after {
        Some(after) => {
            document
                .entries
                .iter()
                .position(|item| item.id == after && !item.retired)
                .ok_or_else(|| anyhow::anyhow!("active neighbor not found: {after}"))?
                + 1
        }
        None => 0,
    };
    document.entries.insert(to, entry);
    Ok(json!({"op": "move_entry", "id": id, "before": from, "after": to}))
}

async fn move_phase(
    tx: &mut Transaction<'_, Postgres>,
    plan: Uuid,
    id: Uuid,
    after: Option<Uuid>,
) -> anyhow::Result<Value> {
    let from = phase_position(tx, plan, id).await?;
    let to = match after {
        Some(after) if id != after => {
            let neighbor = phase_position(tx, plan, after).await?;
            neighbor + i32::from(neighbor < from)
        }
        Some(_) => anyhow::bail!("phase cannot be placed after itself"),
        None => 1,
    };
    super::phases::move_phase(tx, plan, id, from, to).await?;
    Ok(json!({"op": "move_phase", "id": id, "before": from, "after": to}))
}

async fn phase_position(
    tx: &mut Transaction<'_, Postgres>,
    plan: Uuid,
    id: Uuid,
) -> anyhow::Result<i32> {
    sqlx::query_scalar("SELECT position FROM plan_phases WHERE id = $1 AND plan_id = $2")
        .bind(id)
        .bind(plan)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| anyhow::anyhow!("phase not found: {id}"))
}

async fn retire_phase(
    tx: &mut Transaction<'_, Postgres>,
    plan: Uuid,
    id: Uuid,
    successors: Vec<Uuid>,
    document: &mut AuthoredPlan,
    actor: &PlanActor,
) -> anyhow::Result<Value> {
    let result = sqlx::query(
        "UPDATE plan_phases SET status = 'skipped', retired_at = NOW(), status_note = 'Superseded during authoring', updated_at = NOW() \
         WHERE id = $1 AND plan_id = $2 AND status = 'pending' AND started_at IS NULL \
         AND NOT EXISTS (SELECT 1 FROM plan_branch_anchors WHERE parent_phase_id = $1) \
         AND NOT EXISTS (SELECT 1 FROM plan_events WHERE phase_id = $1 AND to_status IN ('in_progress', 'blocked', 'completed'))",
    ).bind(id).bind(plan).execute(&mut **tx).await?;
    if result.rows_affected() == 0 {
        anyhow::bail!("only unstarted, unanchored phases can be retired: {id}");
    }
    insert_event(
        tx,
        plan,
        Some(id),
        "phase_status_changed",
        actor,
        Some("pending"),
        Some("retired"),
        Some("Superseded during authoring"),
    )
    .await?;
    let details = document.phases.entry(id).or_default();
    details.successors = successors.clone();
    Ok(json!({"op": "retire_phase", "id": id, "successors": successors}))
}

async fn validate_evidence_ancestors(
    tx: &mut Transaction<'_, Postgres>,
    plan: Uuid,
    revisions: &BTreeMap<Uuid, i64>,
) -> anyhow::Result<()> {
    let applicable = ancestors(tx, plan).await?;
    if applicable.len() != revisions.len() {
        anyhow::bail!("record evidence with the authoring revisions of each applicable ancestor");
    }
    for ancestor in applicable {
        let (current, _) = read_authoring(tx, ancestor.id, &ancestor.guidance).await?;
        if !revisions
            .get(&ancestor.id)
            .is_some_and(|revision| (0..=current.revision).contains(revision))
        {
            anyhow::bail!(
                "invalid or missing evidence revision for ancestor {}",
                ancestor.id
            );
        }
    }
    Ok(())
}
