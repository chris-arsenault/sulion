use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Pool;

#[derive(Debug, Clone)]
pub struct PlanActor {
    pub kind: String,
    pub pty_session_id: Option<Uuid>,
    pub agent_session_uuid: Option<Uuid>,
}

impl PlanActor {
    pub fn user() -> Self {
        Self {
            kind: "user".to_string(),
            pty_session_id: None,
            agent_session_uuid: None,
        }
    }

    pub async fn for_pty(pool: &Pool, pty_session_id: Uuid) -> anyhow::Result<(Self, String)> {
        let row: Option<(String, Option<Uuid>)> = sqlx::query_as(
            "SELECT repo, current_session_uuid \
               FROM pty_sessions \
              WHERE id = $1 AND state = 'live'",
        )
        .bind(pty_session_id)
        .fetch_optional(pool)
        .await?;
        let (repo, agent_session_uuid) =
            row.ok_or_else(|| anyhow::anyhow!("live PTY session not found: {pty_session_id}"))?;
        Ok((
            Self {
                kind: "agent".to_string(),
                pty_session_id: Some(pty_session_id),
                agent_session_uuid,
            },
            repo,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct NewPhase {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: Option<String>,
    /// Optional t-shirt size ('s' | 'm' | 'l') for weighted burndown.
    #[serde(default)]
    pub size: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreatePlanInput {
    pub repo_name: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub phases: Vec<NewPhase>,
    #[serde(default)]
    pub all_pending: bool,
    #[serde(default = "default_true")]
    pub attach_current_pty: bool,
}

/// Open a sub-plan under `parent_phase_refs` of the acting PTY's current plan
/// (or `parent_plan_id` when given). Anchors accept 1-based positions or phase
/// UUIDs; an empty list anchors to the parent's current phase.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct BranchPlanInput {
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub phases: Vec<NewPhase>,
    #[serde(default)]
    pub all_pending: bool,
    #[serde(default)]
    pub parent_phase_refs: Vec<String>,
    /// Note recorded on the parent phases and the branch event.
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct UpdatePlanInput {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub status: Option<String>,
    pub note: Option<String>,
    #[serde(default)]
    pub skip_remaining: bool,
    /// Set by `plan return`: refuse unless this plan is a branch, so the verb
    /// can never quietly close a root plan.
    #[serde(default)]
    pub require_branch: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct UpdatePhaseInput {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub status_note: Option<String>,
    pub position: Option<i32>,
    pub size: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PlanPhaseView {
    pub id: Uuid,
    pub plan_id: Uuid,
    pub position: i32,
    pub title: String,
    pub description: String,
    pub status: String,
    pub status_note: Option<String>,
    pub size: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PlanAttachmentView {
    pub pty_session_id: Uuid,
    pub agent_session_uuid: Option<Uuid>,
    pub attached_at: DateTime<Utc>,
}

/// One step of the root-to-plan breadcrumb, ordered root first.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PlanAncestorView {
    pub id: Uuid,
    pub title: String,
    pub status: String,
    pub depth: i32,
}

/// A direct sub-plan, with the parent phases it covers.
#[derive(Debug, Clone, Serialize)]
pub struct PlanBranchView {
    pub id: Uuid,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub depth: i32,
    pub total_phases: i32,
    pub completed_phases: i32,
    pub anchor_phase_ids: Vec<Uuid>,
}

/// One plan in a whole tree, root first then depth-ordered.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PlanTreeNodeView {
    pub id: Uuid,
    pub title: String,
    pub status: String,
    pub depth: i32,
    pub parent_plan_id: Option<Uuid>,
    pub total_phases: i32,
    pub completed_phases: i32,
    pub blocked_phases: i32,
    pub attached_pty_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanView {
    pub id: Uuid,
    pub repo_name: String,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub revision: i64,
    pub parent_plan_id: Option<Uuid>,
    pub root_plan_id: Uuid,
    pub depth: i32,
    pub created_by_pty_id: Option<Uuid>,
    pub created_by_agent_session_uuid: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub phases: Vec<PlanPhaseView>,
    pub attachments: Vec<PlanAttachmentView>,
    /// Phases in the parent plan that this plan covers. Empty for a root.
    pub anchor_phase_ids: Vec<Uuid>,
    /// Root first, immediate parent last. Empty for a root.
    pub ancestors: Vec<PlanAncestorView>,
    /// Direct sub-plans, open ones first.
    pub branches: Vec<PlanBranchView>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PlanEventView {
    pub id: i64,
    pub plan_id: Uuid,
    pub phase_id: Option<Uuid>,
    pub event_type: String,
    pub actor_kind: String,
    pub pty_session_id: Option<Uuid>,
    pub agent_session_uuid: Option<Uuid>,
    pub from_status: Option<String>,
    pub to_status: Option<String>,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PlanSummaryView {
    pub id: Uuid,
    pub repo_name: String,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub revision: i64,
    pub total_phases: i32,
    pub completed_phases: i32,
    pub blocked_phases: i32,
    pub current_phase_id: Option<Uuid>,
    pub current_phase_title: Option<String>,
    pub current_phase_status: Option<String>,
    pub attached_pty_ids: Vec<Uuid>,
    pub parent_plan_id: Option<Uuid>,
    pub root_plan_id: Uuid,
    pub depth: i32,
    pub open_branches: i32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(super) struct PlanRow {
    pub(super) id: Uuid,
    pub(super) repo_name: String,
    pub(super) title: String,
    pub(super) summary: String,
    pub(super) status: String,
    pub(super) revision: i64,
    pub(super) parent_plan_id: Option<Uuid>,
    pub(super) root_plan_id: Uuid,
    pub(super) depth: i32,
    pub(super) created_by_pty_id: Option<Uuid>,
    pub(super) created_by_agent_session_uuid: Option<Uuid>,
    pub(super) created_at: DateTime<Utc>,
    pub(super) updated_at: DateTime<Utc>,
    pub(super) closed_at: Option<DateTime<Utc>>,
}

fn default_true() -> bool {
    true
}
