//! Plan trees. A branch is an ordinary plan with a parent and a set of anchor
//! phases in that parent, so every existing plan operation works on it
//! unchanged; what lives here is the hierarchy itself and the pair of moves
//! that make it usable — opening a sub-plan under a step, and unwinding back.
//!
//! Parentage is fixed at creation, and `root_plan_id`/`depth` are derived from
//! the parent at the same moment. That makes cycles structurally impossible and
//! keeps ancestry reads off the recursive path everywhere except the breadcrumb.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::model::{
    BranchPlanInput, PlanActor, PlanAncestorView, PlanBranchView, PlanTreeNodeView, PlanView,
};
use super::{
    attach_in_tx, bump_revision, clean_note, ensure_plan_open, get, insert_event, insert_phases,
    insert_plan, limited_text, required_text, validate_actor, validate_new_phases,
    MAX_SUMMARY_CHARS, MAX_TITLE_CHARS,
};
use crate::db::Pool;

/// Runaway guard, not a product limit: a plan tree deeper than this is an agent
/// looping, not a real decomposition.
const MAX_BRANCH_DEPTH: i32 = 8;

/// Open a sub-plan under one or more phases of `parent_plan_id` and move the
/// acting PTY onto it. Closing the branch pops the PTY back to the parent.
pub async fn branch(
    pool: &Pool,
    parent_plan_id: Uuid,
    input: BranchPlanInput,
    actor: &PlanActor,
) -> anyhow::Result<PlanView> {
    validate_actor(actor)?;
    let title = required_text(&input.title, "plan title", MAX_TITLE_CHARS)?;
    let summary = limited_text(&input.summary, "plan summary", MAX_SUMMARY_CHARS)?;
    let note = clean_note(input.note.as_deref())?;
    let phases = validate_new_phases(&input.phases, input.all_pending)?;

    let mut tx = pool.begin().await?;
    ensure_plan_open(&mut tx, parent_plan_id).await?;
    let (repo_name, parent_depth, root_plan_id): (String, i32, Uuid) =
        sqlx::query_as("SELECT repo_name, depth, root_plan_id FROM plans WHERE id = $1")
            .bind(parent_plan_id)
            .fetch_one(&mut *tx)
            .await?;
    let depth = parent_depth + 1;
    if depth > MAX_BRANCH_DEPTH {
        anyhow::bail!(
            "branch depth {depth} exceeds the maximum of {MAX_BRANCH_DEPTH}; \
             close a branch before opening another"
        );
    }

    let anchors = resolve_anchor_phases(&mut tx, parent_plan_id, &input.parent_phase_refs).await?;

    let plan_id = Uuid::new_v4();
    insert_plan(
        &mut tx,
        plan_id,
        &repo_name,
        &title,
        &summary,
        Some(parent_plan_id),
        root_plan_id,
        depth,
        actor,
    )
    .await?;
    insert_phases(&mut tx, plan_id, phases, actor).await?;

    for anchor in &anchors {
        sqlx::query(
            "INSERT INTO plan_branch_anchors (plan_id, parent_phase_id, created_at) \
             VALUES ($1, $2, NOW())",
        )
        .bind(plan_id)
        .bind(anchor)
        .execute(&mut *tx)
        .await?;
        // A phase whose work has moved into a sub-plan is under way. Anything
        // already in_progress or blocked keeps the status the agent chose.
        sqlx::query(
            "UPDATE plan_phases \
                SET status = 'in_progress', started_at = COALESCE(started_at, NOW()), \
                    updated_at = NOW() \
              WHERE id = $1 AND status = 'pending'",
        )
        .bind(anchor)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            parent_plan_id,
            Some(*anchor),
            "branch_opened",
            actor,
            None,
            None,
            Some(&branch_note(&title, note.as_deref())),
        )
        .await?;
    }
    bump_revision(&mut tx, parent_plan_id).await?;

    if let Some(pty_id) = actor.pty_session_id {
        attach_in_tx(&mut tx, plan_id, pty_id, actor, false).await?;
    }
    tx.commit().await?;
    get(pool, plan_id).await
}

/// Every plan in the tree containing `plan_id`, root first. Ordered so a
/// caller can indent by depth without re-sorting.
pub async fn tree(pool: &Pool, plan_id: Uuid) -> anyhow::Result<Vec<PlanTreeNodeView>> {
    let root_plan_id: Uuid = sqlx::query_scalar("SELECT root_plan_id FROM plans WHERE id = $1")
        .bind(plan_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| anyhow::anyhow!("plan not found: {plan_id}"))?;
    let rows = sqlx::query_as(
        "SELECT p.id, p.title, p.status, p.depth, p.parent_plan_id, \
                COALESCE(stats.total_phases, 0)::INT AS total_phases, \
                COALESCE(stats.completed_phases, 0)::INT AS completed_phases, \
                COALESCE(stats.blocked_phases, 0)::INT AS blocked_phases, \
                COALESCE(attached.pty_ids, ARRAY[]::UUID[]) AS attached_pty_ids \
           FROM plans p \
           LEFT JOIN LATERAL ( \
               SELECT COUNT(*) AS total_phases, \
                      COUNT(*) FILTER (WHERE status IN ('completed', 'skipped')) \
                          AS completed_phases, \
                      COUNT(*) FILTER (WHERE status = 'blocked') AS blocked_phases \
                 FROM plan_phases pp WHERE pp.plan_id = p.id \
           ) stats ON TRUE \
           LEFT JOIN LATERAL ( \
               SELECT ARRAY_AGG(pa.pty_session_id ORDER BY pa.attached_at) AS pty_ids \
                 FROM plan_attachments pa \
                WHERE pa.plan_id = p.id AND pa.detached_at IS NULL \
           ) attached ON TRUE \
          WHERE p.root_plan_id = $1 \
          ORDER BY p.depth, p.created_at",
    )
    .bind(root_plan_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Release a closing plan's PTYs. A branch pops them onto its parent and
/// unblocks any anchor phase the branch was opened to resolve; a root, or a
/// branch whose parent is itself closed, simply detaches.
pub(super) async fn close_branch_into_parent(
    tx: &mut Transaction<'_, Postgres>,
    plan_id: Uuid,
    parent_plan_id: Option<Uuid>,
    status: &str,
    actor: &PlanActor,
    note: Option<&str>,
) -> anyhow::Result<()> {
    let parent_open = match parent_plan_id {
        Some(parent_id) => {
            let parent_status: Option<String> =
                sqlx::query_scalar("SELECT status FROM plans WHERE id = $1")
                    .bind(parent_id)
                    .fetch_optional(&mut **tx)
                    .await?;
            matches!(parent_status.as_deref(), Some("active") | Some("paused"))
        }
        None => false,
    };
    let Some(parent_id) = parent_plan_id.filter(|_| parent_open) else {
        sqlx::query(
            "UPDATE plan_attachments SET detached_at = NOW() \
              WHERE plan_id = $1 AND detached_at IS NULL",
        )
        .bind(plan_id)
        .execute(&mut **tx)
        .await?;
        return Ok(());
    };

    // Completing a branch resolves whatever blocked its anchors by definition.
    // A canceled branch leaves the anchor exactly as the agent left it.
    if status == "completed" {
        let released: Vec<Uuid> = sqlx::query_scalar(
            "UPDATE plan_phases SET status = 'in_progress', updated_at = NOW() \
              WHERE status = 'blocked' \
                AND id IN (SELECT parent_phase_id FROM plan_branch_anchors WHERE plan_id = $1) \
              RETURNING id",
        )
        .bind(plan_id)
        .fetch_all(&mut **tx)
        .await?;
        for phase_id in released {
            insert_event(
                tx,
                parent_id,
                Some(phase_id),
                "phase_status_changed",
                actor,
                Some("blocked"),
                Some("in_progress"),
                Some(note.unwrap_or("branch completed")),
            )
            .await?;
        }
    }

    let ptys: Vec<Uuid> = sqlx::query_scalar(
        "SELECT pty_session_id FROM plan_attachments \
          WHERE plan_id = $1 AND detached_at IS NULL ORDER BY attached_at",
    )
    .bind(plan_id)
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE plan_attachments SET detached_at = NOW() \
          WHERE plan_id = $1 AND detached_at IS NULL",
    )
    .bind(plan_id)
    .execute(&mut **tx)
    .await?;
    for pty_id in ptys {
        // A PTY that exited while its branch was open has nothing to pop to.
        let live: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pty_sessions WHERE id = $1 AND state = 'live')",
        )
        .bind(pty_id)
        .fetch_one(&mut **tx)
        .await?;
        if live {
            attach_in_tx(tx, parent_id, pty_id, actor, false).await?;
        }
    }
    bump_revision(tx, parent_id).await?;
    insert_event(
        tx,
        parent_id,
        None,
        "branch_closed",
        actor,
        None,
        Some(status),
        note,
    )
    .await?;
    Ok(())
}

/// Phases in the parent plan that this plan covers.
pub(super) async fn anchor_phase_ids(pool: &Pool, plan_id: Uuid) -> anyhow::Result<Vec<Uuid>> {
    let rows = sqlx::query_scalar(
        "SELECT a.parent_phase_id \
           FROM plan_branch_anchors a \
           JOIN plan_phases p ON p.id = a.parent_phase_id \
          WHERE a.plan_id = $1 \
          ORDER BY p.position",
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Root first, immediate parent last. Empty for a root plan.
pub(super) async fn ancestors(pool: &Pool, plan_id: Uuid) -> anyhow::Result<Vec<PlanAncestorView>> {
    let rows = sqlx::query_as(
        "WITH RECURSIVE chain AS ( \
             SELECT id, title, status, depth, parent_plan_id \
               FROM plans WHERE id = $1 \
             UNION ALL \
             SELECT p.id, p.title, p.status, p.depth, p.parent_plan_id \
               FROM plans p JOIN chain c ON p.id = c.parent_plan_id \
         ) \
         SELECT id, title, status, depth FROM chain WHERE id <> $1 ORDER BY depth",
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Direct sub-plans of `plan_id`, open first, each with the anchor phases it
/// covers so the caller can hang it off the right step.
pub(super) async fn branch_views(
    pool: &Pool,
    plan_id: Uuid,
) -> anyhow::Result<Vec<PlanBranchView>> {
    let rows: Vec<(Uuid, String, String, String, i32, i32, i32)> = sqlx::query_as(
        "SELECT c.id, c.title, c.summary, c.status, c.depth, \
                COALESCE(stats.total_phases, 0)::INT, \
                COALESCE(stats.completed_phases, 0)::INT \
           FROM plans c \
           LEFT JOIN LATERAL ( \
               SELECT COUNT(*) AS total_phases, \
                      COUNT(*) FILTER (WHERE status IN ('completed', 'skipped')) \
                          AS completed_phases \
                 FROM plan_phases pp WHERE pp.plan_id = c.id \
           ) stats ON TRUE \
          WHERE c.parent_plan_id = $1 \
          ORDER BY CASE WHEN c.status IN ('active', 'paused') THEN 0 ELSE 1 END, \
                   c.created_at",
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, title, summary, status, depth, total_phases, completed_phases) in rows {
        out.push(PlanBranchView {
            id,
            title,
            summary,
            status,
            depth,
            total_phases,
            completed_phases,
            anchor_phase_ids: anchor_phase_ids(pool, id).await?,
        });
    }
    Ok(out)
}

/// Anchor phases for a new branch. Explicit references win; with none given the
/// parent's current phase is used, matching what `plan current` reports.
async fn resolve_anchor_phases(
    tx: &mut Transaction<'_, Postgres>,
    parent_plan_id: Uuid,
    refs: &[String],
) -> anyhow::Result<Vec<Uuid>> {
    if refs.is_empty() {
        let current: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM plan_phases \
              WHERE plan_id = $1 AND status IN ('in_progress', 'blocked') \
              ORDER BY CASE status WHEN 'blocked' THEN 0 ELSE 1 END, position LIMIT 1",
        )
        .bind(parent_plan_id)
        .fetch_optional(&mut **tx)
        .await?;
        let phase_id = current.ok_or_else(|| {
            anyhow::anyhow!(
                "parent plan has no phase in progress; name the phases with parent_phase_refs"
            )
        })?;
        return Ok(vec![phase_id]);
    }
    let mut out: Vec<Uuid> = Vec::with_capacity(refs.len());
    for reference in refs {
        let phase_id = resolve_phase_id_in_tx(tx, parent_plan_id, reference).await?;
        if !out.contains(&phase_id) {
            out.push(phase_id);
        }
    }
    Ok(out)
}

async fn resolve_phase_id_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    plan_id: Uuid,
    reference: &str,
) -> anyhow::Result<Uuid> {
    if let Ok(id) = Uuid::parse_str(reference) {
        return sqlx::query_scalar("SELECT id FROM plan_phases WHERE plan_id = $1 AND id = $2")
            .bind(plan_id)
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| anyhow::anyhow!("phase not found in parent plan: {reference}"));
    }
    let position: i32 = reference
        .parse()
        .map_err(|_| anyhow::anyhow!("phase must be a UUID or 1-based position"))?;
    sqlx::query_scalar("SELECT id FROM plan_phases WHERE plan_id = $1 AND position = $2")
        .bind(plan_id)
        .bind(position)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| anyhow::anyhow!("phase not found at position {position}"))
}

fn branch_note(title: &str, note: Option<&str>) -> String {
    match note {
        Some(note) => format!("branched to \"{title}\": {note}"),
        None => format!("branched to \"{title}\""),
    }
}
