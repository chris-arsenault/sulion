use super::{plans, ControlRequest, Pool, Uuid, Value};

fn with_urls(mut plan: Value) -> Value {
    if let Some(base) = crate::config::env_optional("SULION_PLAN_MCP_URL")
        .and_then(|s| url::Url::parse(&s).ok())
        .and_then(|url| url.join("/").ok())
    {
        plans::diagrams::add_plan_urls(&mut plan, &base);
    }
    plan
}

pub(super) async fn dispatch(
    pool: &Pool,
    pty_id: Uuid,
    repo_name: &str,
    request: ControlRequest,
) -> anyhow::Result<Value> {
    match request {
        ControlRequest::PlanCurrent => Ok(with_urls(serde_json::to_value(
            plans::current_for_pty(pool, pty_id).await?,
        )?)),
        ControlRequest::PlanDiagramList => {
            let diagrams = plans::diagrams::list(pool, repo_name).await?;
            Ok(
                serde_json::json!({"diagrams": diagrams.iter().map(plans::diagrams::Diagram::result).collect::<Vec<_>>()}),
            )
        }
        ControlRequest::PlanDiagramRegister { diagram } => {
            let registered = plans::diagrams::register(
                pool,
                repo_name,
                diagram.workspace_id,
                &diagram.room_id,
                &diagram.title,
            )
            .await?;
            Ok(serde_json::json!({"diagram": registered.result()}))
        }
        ControlRequest::PlanList { include_closed } => Ok(serde_json::to_value(
            plans::list_for_repo(pool, repo_name, include_closed).await?,
        )?),
        ControlRequest::PlanLookup { title } => Ok(serde_json::to_value(
            plans::lookup(pool, Some(repo_name), &title).await?,
        )?),

        _ => read(pool, pty_id, repo_name, request).await,
    }
}

async fn read(
    pool: &Pool,
    pty_id: Uuid,
    repo_name: &str,
    request: ControlRequest,
) -> anyhow::Result<Value> {
    let plan_id = match &request {
        ControlRequest::PlanShow { plan_id } | ControlRequest::PlanReview { plan_id, .. } => {
            *plan_id
        }
        _ => unreachable!("called for plan reads"),
    };
    let id = plans::resolve_plan_id(pool, pty_id, plan_id, repo_name).await?;
    let plan = plans::get(pool, id).await?;
    match request {
        ControlRequest::PlanReview { phase_id, .. } => {
            use plans::PhaseReference;
            let phase_id = match phase_id {
                Some(PhaseReference::Id(id)) => Some(id),
                Some(PhaseReference::Position(position)) => Some(
                    plan.phases
                        .iter()
                        .find(|phase| phase.position == position)
                        .ok_or_else(|| anyhow::anyhow!("phase not found at position {position}"))?
                        .id,
                ),
                None => None,
            };
            let mut review = plans::review::review(plan, phase_id)?;
            review["plan"] = with_urls(review["plan"].take());
            Ok(review)
        }
        _ => Ok(with_urls(serde_json::to_value(plan)?)),
    }
}
