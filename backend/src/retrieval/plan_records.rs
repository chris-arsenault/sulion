use super::*;
use crate::plans::{self, PlanEventView, PlanView};

#[derive(Debug, Serialize, sqlx::FromRow)]
pub(super) struct PlanMatch {
    plan_id: Uuid,
    repo: String,
    title: String,
    summary: String,
    status: String,
    root_plan_id: Uuid,
    phase_id: Option<Uuid>,
    phase_title: Option<String>,
    agent_session_uuid: Option<Uuid>,
    turn_id: Option<i64>,
    score: f32,
}

pub(super) struct PlanSearch<'a> {
    pub(super) text: &'a str,
    pub(super) repo: Option<&'a str>,
    pub(super) session: Option<Uuid>,
    pub(super) since: Option<DateTime<Utc>>,
    pub(super) until: Option<DateTime<Utc>>,
    pub(super) matching_turns: Vec<(Uuid, i64, f32)>,
    pub(super) limit: i64,
    pub(super) lexical: bool,
    pub(super) agent: Option<&'a str>,
    pub(super) model: Option<&'a str>,
}

pub(super) async fn find_plans(
    pool: &Pool,
    query: PlanSearch<'_>,
) -> Result<Vec<PlanMatch>, RetrievalError> {
    if !query.lexical && query.matching_turns.is_empty() {
        return Ok(Vec::new());
    }
    let sessions: Vec<_> = query.matching_turns.iter().map(|turn| turn.0).collect();
    let turns: Vec<_> = query.matching_turns.iter().map(|turn| turn.1).collect();
    let scores: Vec<_> = query.matching_turns.iter().map(|turn| turn.2).collect();
    Ok(sqlx::query_as(include_str!("plan_search.sql"))
        .bind(query.text)
        .bind(query.repo)
        .bind(query.session)
        .bind(query.since)
        .bind(query.until)
        .bind(sessions)
        .bind(turns)
        .bind(scores)
        .bind(query.limit)
        .bind(query.lexical)
        .bind(query.agent)
        .bind(query.model)
        .fetch_all(pool)
        .await?)
}

#[derive(Debug, Deserialize)]
pub(super) struct PlanQuery {
    plan_id: Uuid,
    phase_id: Option<Uuid>,
    history: Option<bool>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct PlanTurn {
    phase_id: Option<Uuid>,
    agent_session_uuid: Uuid,
    turn_id: i64,
    timestamp: DateTime<Utc>,
    preview: String,
    kinds: Vec<String>,
    event_ids: Vec<i64>,
}

#[derive(Debug, Serialize)]
pub(super) struct PlanRecord {
    plan: PlanView,
    phase_id: Option<Uuid>,
    turns: Vec<PlanTurn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    history: Option<Vec<PlanEventView>>,
}

pub(super) async fn plan_route(
    State(state): State<Arc<RetrievalState>>,
    Query(query): Query<PlanQuery>,
) -> Result<Json<PlanRecord>, RetrievalError> {
    let mut tx = plans::read_snapshot(&state.pool).await?;
    let mut plan = plans::read_plan(&mut tx, query.plan_id)
        .await?
        .ok_or_else(|| not_found(format!("plan not found: {}", query.plan_id)))?;
    if let Some(phase_id) = query.phase_id {
        plan.phases.retain(|phase| phase.id == phase_id);
        if plan.phases.is_empty() {
            return Err(not_found(format!(
                "phase {phase_id} not found in plan {}",
                plan.id
            )));
        }
        plan.branches
            .retain(|branch| branch.anchor_phase_ids.contains(&phase_id));
        if let Some(authoring) = plan.authoring.as_mut() {
            authoring.document.phases.retain(|id, _| *id == phase_id);
            authoring.needs_review.retain(|id, _| *id == phase_id);
        }
    }
    let turns = plan_turns(&mut tx, query.plan_id, query.phase_id).await?;
    let history = if query.history.unwrap_or(false) {
        let mut events = plans::read_events(&mut *tx, query.plan_id).await?;
        if let Some(phase_id) = query.phase_id {
            let phase_key = phase_id.to_string();
            events
                .retain(|event| event.phase_id == Some(phase_id) || edits_phase(event, &phase_key));
        }
        Some(events)
    } else {
        None
    };
    tx.commit().await?;
    Ok(Json(PlanRecord {
        plan,
        phase_id: query.phase_id,
        turns,
        history,
    }))
}

async fn plan_turns(
    connection: &mut sqlx::PgConnection,
    plan_id: Uuid,
    phase_id: Option<Uuid>,
) -> Result<Vec<PlanTurn>, RetrievalError> {
    Ok(sqlx::query_as(
        "SELECT l.phase_id, t.session_uuid AS agent_session_uuid, t.turn_id, \
                t.start_timestamp AS timestamp, t.preview, \
                array_agg(DISTINCT l.kind ORDER BY l.kind) AS kinds, \
                COALESCE(array_agg(DISTINCT l.event_id ORDER BY l.event_id) \
                    FILTER (WHERE l.event_id IS NOT NULL), '{}') AS event_ids \
           FROM plan_turn_provenance($1) l JOIN timeline_turns t USING (session_uuid, turn_id) \
          WHERE ($2::UUID IS NULL OR l.phase_id = $2) \
          GROUP BY l.phase_id, t.session_uuid, t.turn_id \
          ORDER BY t.start_timestamp, t.session_uuid, t.turn_id, l.phase_id NULLS FIRST",
    )
    .bind(plan_id)
    .bind(phase_id)
    .fetch_all(connection)
    .await?)
}

fn edits_phase(event: &PlanEventView, phase_id: &str) -> bool {
    event
        .authoring_change
        .as_ref()
        .and_then(|change| change["changes"].as_array())
        .is_some_and(|operations| {
            operations.iter().any(|op| {
                op["id"].as_str() == Some(phase_id)
                    && matches!(
                        op["op"].as_str(),
                        Some(
                            "edit_phase"
                                | "add_phase"
                                | "move_phase"
                                | "retire_phase"
                                | "record_evidence"
                        )
                    )
            })
        })
}

fn not_found(message: String) -> RetrievalError {
    RetrievalError {
        status: StatusCode::NOT_FOUND,
        message,
    }
}
