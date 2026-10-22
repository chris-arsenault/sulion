use super::{call, guidance, print_plan_data, ControlRequest, Value};

// Keep JSON stdout compatible with existing consumers. Its recurring reminder
// goes to stderr, including commands whose data is an array (list/history/tree).
fn print_reminder(plan: &Value, stderr: bool) {
    let text = guidance::format_reminder(plan);
    if stderr {
        eprint!("{text}");
    } else {
        print!("{text}");
    }
}

pub(super) async fn print_attached(stderr: bool) {
    print_from_request(ControlRequest::PlanCurrent, stderr).await;
}

async fn print_from_request(request: ControlRequest, stderr: bool) {
    match call(request).await {
        Ok(response) if response.ok => {
            if let Some(plan) = response.data {
                print_reminder(&plan, stderr);
            }
        }
        Ok(response) => eprintln!(
            "Plan reminder unavailable: {}",
            response
                .error
                .as_deref()
                .unwrap_or("no plan context returned")
        ),
        Err(err) => eprintln!("Plan reminder unavailable: {err}"),
    }
}

fn context_request(request: &ControlRequest) -> ControlRequest {
    let plan_id = match request {
        ControlRequest::PlanAttach { plan_id } => Some(*plan_id),
        ControlRequest::PlanBranch { plan_id, .. }
        | ControlRequest::PlanTree { plan_id }
        | ControlRequest::PlanShow { plan_id }
        | ControlRequest::PlanUpdate { plan_id, .. }
        | ControlRequest::PlanAddPhase { plan_id, .. }
        | ControlRequest::PlanUpdatePhase { plan_id, .. }
        | ControlRequest::PlanDetach { plan_id }
        | ControlRequest::PlanHistory { plan_id } => *plan_id,
        _ => None,
    };
    ControlRequest::PlanShow { plan_id }
}

pub(super) async fn run(
    request: ControlRequest,
    json: bool,
    full_guidance: bool,
) -> anyhow::Result<i32> {
    let context = context_request(&request);
    let response = match call(request).await {
        Ok(response) => response,
        Err(err) => {
            eprintln!("sulion plan: {err}\nnext: sulion plan help\nPlan reminder unavailable: control service could not be reached.");
            return Ok(65);
        }
    };
    if !response.ok {
        eprintln!(
            "sulion plan: {}\nnext: sulion plan list --all",
            response
                .error
                .as_deref()
                .unwrap_or("control request failed")
        );
        print_from_request(context, true).await;
        return Ok(66);
    }
    let data = response.data.unwrap_or(Value::Null);
    // Mutations return their target plan, even when closing/detaching it has
    // changed the PTY attachment. Never substitute an unrelated current plan.
    if data.get("phases").is_some() {
        if json || !full_guidance {
            print_reminder(&data, json);
        }
    } else {
        print_from_request(context, json).await;
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&data)?);
    } else {
        print_plan_data(&data, full_guidance);
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn context_follows_explicit_targets_and_current_attachment() {
        let id = Uuid::new_v4();
        for request in [
            ControlRequest::PlanHistory { plan_id: Some(id) },
            ControlRequest::PlanTree { plan_id: Some(id) },
            ControlRequest::PlanDetach { plan_id: Some(id) },
            ControlRequest::PlanAttach { plan_id: id },
        ] {
            assert_eq!(
                context_request(&request),
                ControlRequest::PlanShow { plan_id: Some(id) }
            );
        }
        assert_eq!(
            context_request(&ControlRequest::PlanList {
                include_closed: false
            }),
            ControlRequest::PlanShow { plan_id: None }
        );
    }
}
