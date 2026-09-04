use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::db::Pool;

mod branches;
mod model;
mod phases;

use branches::{ancestors, anchor_phase_ids, branch_views, close_branch_into_parent};
pub use branches::{branch, tree};
use model::PlanRow;
pub use model::{
    BranchPlanInput, CreatePlanInput, NewPhase, PlanActor, PlanAncestorView, PlanAttachmentView,
    PlanBranchView, PlanEventView, PlanPhaseView, PlanSummaryView, PlanTreeNodeView, PlanView,
    UpdatePhaseInput, UpdatePlanInput,
};
pub use phases::{add_phase, resolve_phase_id, update_phase};
use phases::{initial_phase_status, skip_remaining_phases, validate_phase_size};

const MAX_TITLE_CHARS: usize = 160;
const MAX_SUMMARY_CHARS: usize = 1_000;
const MAX_DESCRIPTION_CHARS: usize = 1_000;
const MAX_NOTE_CHARS: usize = 1_000;

const PLAN_COLUMNS: &str = "id, repo_name, title, summary, status, revision, \
     parent_plan_id, root_plan_id, depth, \
     created_by_pty_id, created_by_agent_session_uuid, \
     created_at, updated_at, closed_at";

pub async fn create(
    pool: &Pool,
    input: CreatePlanInput,
    actor: &PlanActor,
) -> anyhow::Result<PlanView> {
    validate_actor(actor)?;
    let repo_name = required_text(&input.repo_name, "repo name", MAX_TITLE_CHARS)?;
    let title = required_text(&input.title, "plan title", MAX_TITLE_CHARS)?;
    let summary = limited_text(&input.summary, "plan summary", MAX_SUMMARY_CHARS)?;
    let phases = validate_new_phases(&input.phases, input.all_pending)?;

    let plan_id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    insert_plan(
        &mut tx, plan_id, &repo_name, &title, &summary, None, plan_id, 0, actor,
    )
    .await?;
    insert_phases(&mut tx, plan_id, phases, actor).await?;

    if input.attach_current_pty {
        if let Some(pty_id) = actor.pty_session_id {
            attach_in_tx(&mut tx, plan_id, pty_id, actor, false).await?;
        }
    }
    tx.commit().await?;
    get(pool, plan_id).await
}

pub async fn get(pool: &Pool, plan_id: Uuid) -> anyhow::Result<PlanView> {
    let row: PlanRow = sqlx::query_as(&format!("SELECT {PLAN_COLUMNS} FROM plans WHERE id = $1"))
        .bind(plan_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| anyhow::anyhow!("plan not found: {plan_id}"))?;
    hydrate(pool, row).await
}

pub async fn current_for_pty(pool: &Pool, pty_session_id: Uuid) -> anyhow::Result<PlanView> {
    let plan_id: Uuid = sqlx::query_scalar(
        "SELECT plan_id FROM plan_attachments \
          WHERE pty_session_id = $1 AND detached_at IS NULL",
    )
    .bind(pty_session_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| anyhow::anyhow!("no plan is attached to this PTY"))?;
    get(pool, plan_id).await
}

pub async fn list_for_repo(
    pool: &Pool,
    repo_name: &str,
    include_closed: bool,
) -> anyhow::Result<Vec<PlanView>> {
    let rows: Vec<PlanRow> = sqlx::query_as(&format!(
        "SELECT {PLAN_COLUMNS} \
           FROM plans \
          WHERE repo_name = $1 \
            AND ($2 OR status IN ('active', 'paused')) \
          ORDER BY CASE status WHEN 'active' THEN 0 WHEN 'paused' THEN 1 ELSE 2 END, \
                   updated_at DESC",
    ))
    .bind(repo_name)
    .bind(include_closed)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(hydrate(pool, row).await?);
    }
    Ok(out)
}

pub async fn list_open_summaries(pool: &Pool) -> anyhow::Result<Vec<PlanSummaryView>> {
    let rows = sqlx::query_as(
        "SELECT p.id, p.repo_name, p.title, p.summary, p.status, p.revision, \
                COALESCE(stats.total_phases, 0)::INT AS total_phases, \
                COALESCE(stats.completed_phases, 0)::INT AS completed_phases, \
                COALESCE(stats.blocked_phases, 0)::INT AS blocked_phases, \
                current_phase.id AS current_phase_id, \
                current_phase.title AS current_phase_title, \
                current_phase.status AS current_phase_status, \
                COALESCE(attached.pty_ids, ARRAY[]::UUID[]) AS attached_pty_ids, \
                p.parent_plan_id, p.root_plan_id, p.depth, \
                COALESCE(branches.open_branches, 0)::INT AS open_branches, \
                p.updated_at \
           FROM plans p \
           LEFT JOIN LATERAL ( \
               SELECT COUNT(*) AS total_phases, \
                      COUNT(*) FILTER (WHERE status = 'completed') AS completed_phases, \
                      COUNT(*) FILTER (WHERE status = 'blocked') AS blocked_phases \
                 FROM plan_phases pp WHERE pp.plan_id = p.id \
           ) stats ON TRUE \
           LEFT JOIN LATERAL ( \
               SELECT id, title, status \
                 FROM plan_phases pp \
                WHERE pp.plan_id = p.id \
                  AND pp.status IN ('in_progress', 'blocked') \
                ORDER BY CASE pp.status WHEN 'blocked' THEN 0 ELSE 1 END, pp.position \
                LIMIT 1 \
           ) current_phase ON TRUE \
           LEFT JOIN LATERAL ( \
               SELECT ARRAY_AGG(pa.pty_session_id ORDER BY pa.attached_at) AS pty_ids \
                 FROM plan_attachments pa \
                WHERE pa.plan_id = p.id AND pa.detached_at IS NULL \
           ) attached ON TRUE \
           LEFT JOIN LATERAL ( \
               SELECT COUNT(*) AS open_branches \
                 FROM plans child \
                WHERE child.parent_plan_id = p.id \
                  AND child.status IN ('active', 'paused') \
           ) branches ON TRUE \
          WHERE p.status IN ('active', 'paused') \
          ORDER BY p.repo_name, p.updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn update(
    pool: &Pool,
    plan_id: Uuid,
    input: UpdatePlanInput,
    actor: &PlanActor,
) -> anyhow::Result<PlanView> {
    validate_actor(actor)?;
    let mut tx = pool.begin().await?;
    let current: (String, String, String, Option<Uuid>) = sqlx::query_as(
        "SELECT title, summary, status, parent_plan_id FROM plans WHERE id = $1 FOR UPDATE",
    )
    .bind(plan_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("plan not found: {plan_id}"))?;
    let parent_plan_id = current.3;
    let title = match input.title.as_deref() {
        Some(value) => required_text(value, "plan title", MAX_TITLE_CHARS)?,
        None => current.0,
    };
    let summary = match input.summary.as_deref() {
        Some(value) => limited_text(value, "plan summary", MAX_SUMMARY_CHARS)?,
        None => current.1,
    };
    let status = input.status.as_deref().unwrap_or(&current.2);
    validate_plan_status(status)?;
    let note = clean_note(input.note.as_deref())?;

    if input.require_branch && parent_plan_id.is_none() {
        anyhow::bail!("this plan is not a branch; close it with plan close instead of plan return");
    }

    // A closed parent would strand its open branches: they stay attachable and
    // reachable but can never pop back. Close the tree bottom-up instead.
    if matches!(status, "completed" | "canceled") {
        let open_branches: Vec<String> = sqlx::query_scalar(
            "SELECT title FROM plans \
              WHERE parent_plan_id = $1 AND status IN ('active', 'paused') \
              ORDER BY created_at",
        )
        .bind(plan_id)
        .fetch_all(&mut *tx)
        .await?;
        if !open_branches.is_empty() {
            anyhow::bail!(
                "plan has {} open branch(es): {}; close them first",
                open_branches.len(),
                open_branches.join(", ")
            );
        }
    }

    if status == "completed" {
        let unfinished: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::BIGINT FROM plan_phases \
              WHERE plan_id = $1 AND status NOT IN ('completed', 'skipped')",
        )
        .bind(plan_id)
        .fetch_one(&mut *tx)
        .await?;
        if unfinished > 0 && !input.skip_remaining {
            anyhow::bail!(
                "plan has {unfinished} unfinished phase(s); use skip_remaining to complete it"
            );
        }
    }
    if matches!(status, "completed" | "canceled") && input.skip_remaining {
        skip_remaining_phases(&mut tx, plan_id, actor, note.as_deref()).await?;
    }

    sqlx::query(
        "UPDATE plans \
            SET title = $2, summary = $3, status = $4, revision = revision + 1, \
                closed_at = CASE WHEN $4 IN ('completed', 'canceled') THEN COALESCE(closed_at, NOW()) ELSE NULL END, \
                updated_at = NOW() \
          WHERE id = $1",
    )
    .bind(plan_id)
    .bind(title)
    .bind(summary)
    .bind(status)
    .execute(&mut *tx)
    .await?;

    let event_type = if status != current.2 {
        if matches!(status, "completed" | "canceled") {
            "plan_closed"
        } else {
            "plan_status_changed"
        }
    } else {
        "plan_updated"
    };
    insert_event(
        &mut tx,
        plan_id,
        None,
        event_type,
        actor,
        Some(&current.2),
        Some(status),
        note.as_deref(),
    )
    .await?;

    if matches!(status, "completed" | "canceled") {
        close_branch_into_parent(
            &mut tx,
            plan_id,
            parent_plan_id,
            status,
            actor,
            note.as_deref(),
        )
        .await?;
    }
    tx.commit().await?;
    get(pool, plan_id).await
}

pub async fn attach(
    pool: &Pool,
    plan_id: Uuid,
    pty_session_id: Uuid,
    actor: &PlanActor,
) -> anyhow::Result<PlanView> {
    validate_actor(actor)?;
    let mut tx = pool.begin().await?;
    ensure_plan_open(&mut tx, plan_id).await?;
    attach_in_tx(&mut tx, plan_id, pty_session_id, actor, true).await?;
    tx.commit().await?;
    get(pool, plan_id).await
}

pub async fn detach(
    pool: &Pool,
    plan_id: Uuid,
    pty_session_id: Uuid,
    actor: &PlanActor,
) -> anyhow::Result<PlanView> {
    validate_actor(actor)?;
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        "UPDATE plan_attachments SET detached_at = NOW() \
          WHERE plan_id = $1 AND pty_session_id = $2 AND detached_at IS NULL",
    )
    .bind(plan_id)
    .bind(pty_session_id)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() > 0 {
        bump_revision(&mut tx, plan_id).await?;
        insert_event(
            &mut tx,
            plan_id,
            None,
            "plan_detached",
            actor,
            None,
            None,
            Some(&format!("PTY {pty_session_id}")),
        )
        .await?;
    }
    tx.commit().await?;
    get(pool, plan_id).await
}

pub async fn events(pool: &Pool, plan_id: Uuid) -> anyhow::Result<Vec<PlanEventView>> {
    let rows = sqlx::query_as(
        "SELECT id, plan_id, phase_id, event_type, actor_kind, pty_session_id, \
                agent_session_uuid, from_status, to_status, note, created_at \
           FROM plan_events WHERE plan_id = $1 \
          ORDER BY created_at DESC, id DESC",
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn resolve_plan_id(
    pool: &Pool,
    pty_session_id: Uuid,
    requested: Option<Uuid>,
    repo_name: &str,
) -> anyhow::Result<Uuid> {
    let id = if let Some(id) = requested {
        id
    } else {
        sqlx::query_scalar(
            "SELECT plan_id FROM plan_attachments \
              WHERE pty_session_id = $1 AND detached_at IS NULL",
        )
        .bind(pty_session_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no plan is attached to this PTY"))?
    };
    let belongs_to_repo: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM plans WHERE id = $1 AND repo_name = $2)")
            .bind(id)
            .bind(repo_name)
            .fetch_one(pool)
            .await?;
    if !belongs_to_repo {
        anyhow::bail!("plan not found in current repo: {id}");
    }
    Ok(id)
}

async fn hydrate(pool: &Pool, row: PlanRow) -> anyhow::Result<PlanView> {
    let phases = sqlx::query_as(
        "SELECT id, plan_id, position, title, description, status, status_note, size, \
                started_at, completed_at, created_at, updated_at \
           FROM plan_phases WHERE plan_id = $1 ORDER BY position",
    )
    .bind(row.id)
    .fetch_all(pool)
    .await?;
    let attachments = sqlx::query_as(
        "SELECT pty_session_id, agent_session_uuid, attached_at \
           FROM plan_attachments \
          WHERE plan_id = $1 AND detached_at IS NULL \
          ORDER BY attached_at",
    )
    .bind(row.id)
    .fetch_all(pool)
    .await?;
    let anchor_phase_ids = anchor_phase_ids(pool, row.id).await?;
    let ancestors = if row.parent_plan_id.is_some() {
        ancestors(pool, row.id).await?
    } else {
        Vec::new()
    };
    let branches = branch_views(pool, row.id).await?;
    Ok(PlanView {
        id: row.id,
        repo_name: row.repo_name,
        title: row.title,
        summary: row.summary,
        status: row.status,
        revision: row.revision,
        parent_plan_id: row.parent_plan_id,
        root_plan_id: row.root_plan_id,
        depth: row.depth,
        created_by_pty_id: row.created_by_pty_id,
        created_by_agent_session_uuid: row.created_by_agent_session_uuid,
        created_at: row.created_at,
        updated_at: row.updated_at,
        closed_at: row.closed_at,
        phases,
        attachments,
        anchor_phase_ids,
        ancestors,
        branches,
    })
}

/// Title, description, initial status, and optional size — validated and ready
/// to insert.
type CheckedPhase = (String, String, String, Option<String>);

fn validate_new_phases(
    phases: &[NewPhase],
    all_pending: bool,
) -> anyhow::Result<Vec<CheckedPhase>> {
    if phases.is_empty() {
        anyhow::bail!("a published plan requires at least one phase");
    }
    let mut out = Vec::with_capacity(phases.len());
    for (index, phase) in phases.iter().enumerate() {
        out.push((
            required_text(&phase.title, "phase title", MAX_TITLE_CHARS)?,
            limited_text(
                &phase.description,
                "phase description",
                MAX_DESCRIPTION_CHARS,
            )?,
            initial_phase_status(phase.status.as_deref(), index, all_pending)?,
            validate_phase_size(phase.size.as_deref())?,
        ));
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
async fn insert_plan(
    tx: &mut Transaction<'_, Postgres>,
    plan_id: Uuid,
    repo_name: &str,
    title: &str,
    summary: &str,
    parent_plan_id: Option<Uuid>,
    root_plan_id: Uuid,
    depth: i32,
    actor: &PlanActor,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO plans \
             (id, repo_name, title, summary, status, revision, parent_plan_id, \
              root_plan_id, depth, created_by_pty_id, created_by_agent_session_uuid, \
              created_at, updated_at) \
         VALUES ($1, $2, $3, $4, 'active', 1, $5, $6, $7, $8, $9, NOW(), NOW())",
    )
    .bind(plan_id)
    .bind(repo_name)
    .bind(title)
    .bind(summary)
    .bind(parent_plan_id)
    .bind(root_plan_id)
    .bind(depth)
    .bind(actor.pty_session_id)
    .bind(actor.agent_session_uuid)
    .execute(&mut **tx)
    .await?;
    insert_event(
        tx,
        plan_id,
        None,
        if parent_plan_id.is_some() {
            "branch_created"
        } else {
            "plan_created"
        },
        actor,
        None,
        Some("active"),
        None,
    )
    .await?;
    Ok(())
}

async fn insert_phases(
    tx: &mut Transaction<'_, Postgres>,
    plan_id: Uuid,
    phases: Vec<CheckedPhase>,
    actor: &PlanActor,
) -> anyhow::Result<()> {
    for (index, (title, description, status, size)) in phases.into_iter().enumerate() {
        let phase_id = Uuid::new_v4();
        let started = status == "in_progress";
        sqlx::query(
            "INSERT INTO plan_phases \
                 (id, plan_id, position, title, description, status, size, started_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $8, CASE WHEN $7 THEN NOW() ELSE NULL END, NOW(), NOW())",
        )
        .bind(phase_id)
        .bind(plan_id)
        .bind(index as i32 + 1)
        .bind(title)
        .bind(description)
        .bind(&status)
        .bind(started)
        .bind(size)
        .execute(&mut **tx)
        .await?;
        insert_event(
            tx,
            plan_id,
            Some(phase_id),
            "phase_added",
            actor,
            None,
            Some(&status),
            None,
        )
        .await?;
    }
    Ok(())
}

async fn attach_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    plan_id: Uuid,
    pty_session_id: Uuid,
    actor: &PlanActor,
    bump: bool,
) -> anyhow::Result<()> {
    let plan_repo: String = sqlx::query_scalar("SELECT repo_name FROM plans WHERE id = $1")
        .bind(plan_id)
        .fetch_one(&mut **tx)
        .await?;
    let (pty_repo, current_session): (String, Option<Uuid>) = sqlx::query_as(
        "SELECT repo, current_session_uuid FROM pty_sessions \
          WHERE id = $1 AND state = 'live'",
    )
    .bind(pty_session_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("live PTY session not found: {pty_session_id}"))?;
    if pty_repo != plan_repo {
        anyhow::bail!("cannot attach PTY from repo {pty_repo} to plan in repo {plan_repo}");
    }
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT plan_id FROM plan_attachments \
          WHERE pty_session_id = $1 AND detached_at IS NULL",
    )
    .bind(pty_session_id)
    .fetch_optional(&mut **tx)
    .await?;
    if existing == Some(plan_id) {
        return Ok(());
    }
    if let Some(old_plan_id) = existing {
        sqlx::query(
            "UPDATE plan_attachments SET detached_at = NOW() \
              WHERE pty_session_id = $1 AND detached_at IS NULL",
        )
        .bind(pty_session_id)
        .execute(&mut **tx)
        .await?;
        bump_revision(tx, old_plan_id).await?;
        insert_event(
            tx,
            old_plan_id,
            None,
            "plan_detached",
            actor,
            None,
            None,
            Some(&format!("PTY {pty_session_id} attached to another plan")),
        )
        .await?;
    }
    sqlx::query(
        "INSERT INTO plan_attachments \
             (id, plan_id, pty_session_id, agent_session_uuid, attached_at) \
         VALUES ($1, $2, $3, $4, NOW())",
    )
    .bind(Uuid::new_v4())
    .bind(plan_id)
    .bind(pty_session_id)
    .bind(current_session)
    .execute(&mut **tx)
    .await?;
    if bump {
        bump_revision(tx, plan_id).await?;
    }
    insert_event(
        tx,
        plan_id,
        None,
        "plan_attached",
        actor,
        None,
        None,
        Some(&format!("PTY {pty_session_id}")),
    )
    .await?;
    Ok(())
}

async fn ensure_plan_open(tx: &mut Transaction<'_, Postgres>, plan_id: Uuid) -> anyhow::Result<()> {
    let status: String = sqlx::query_scalar("SELECT status FROM plans WHERE id = $1 FOR UPDATE")
        .bind(plan_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| anyhow::anyhow!("plan not found: {plan_id}"))?;
    if matches!(status.as_str(), "completed" | "canceled") {
        anyhow::bail!("plan is closed with status {status}");
    }
    Ok(())
}

async fn bump_revision(tx: &mut Transaction<'_, Postgres>, plan_id: Uuid) -> anyhow::Result<()> {
    sqlx::query("UPDATE plans SET revision = revision + 1, updated_at = NOW() WHERE id = $1")
        .bind(plan_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_event(
    tx: &mut Transaction<'_, Postgres>,
    plan_id: Uuid,
    phase_id: Option<Uuid>,
    event_type: &str,
    actor: &PlanActor,
    from_status: Option<&str>,
    to_status: Option<&str>,
    note: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO plan_events \
             (plan_id, phase_id, event_type, actor_kind, pty_session_id, \
              agent_session_uuid, from_status, to_status, note, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())",
    )
    .bind(plan_id)
    .bind(phase_id)
    .bind(event_type)
    .bind(&actor.kind)
    .bind(actor.pty_session_id)
    .bind(actor.agent_session_uuid)
    .bind(from_status)
    .bind(to_status)
    .bind(note)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn required_text(value: &str, label: &str, max_chars: usize) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("{label} is required");
    }
    limited_text(value, label, max_chars)
}

fn limited_text(value: &str, label: &str, max_chars: usize) -> anyhow::Result<String> {
    let value = value.trim();
    if value.chars().count() > max_chars {
        anyhow::bail!("{label} exceeds {max_chars} characters");
    }
    Ok(value.to_string())
}

fn clean_note(value: Option<&str>) -> anyhow::Result<Option<String>> {
    value
        .map(|value| limited_text(value, "note", MAX_NOTE_CHARS))
        .transpose()
        .map(|value| value.filter(|value| !value.is_empty()))
}

fn validate_plan_status(status: &str) -> anyhow::Result<()> {
    if matches!(status, "active" | "paused" | "completed" | "canceled") {
        Ok(())
    } else {
        anyhow::bail!("invalid plan status: {status}")
    }
}

fn validate_actor(actor: &PlanActor) -> anyhow::Result<()> {
    if matches!(actor.kind.as_str(), "agent" | "user" | "system") {
        Ok(())
    } else {
        anyhow::bail!("invalid plan actor kind: {}", actor.kind)
    }
}
