use super::{
    call, reject_unknown_options, reminder, take_option, take_uuid_option, ControlRequest,
};
use crate::plans::{
    diagrams::{Diagram, DiagramPhaseLink},
    DirectEdit, PlanEdit,
};
use anyhow::{anyhow, bail, ensure, Context};
use serde_json::Value;

async fn data(request: ControlRequest) -> anyhow::Result<Value> {
    let response = call(request).await?;
    ensure!(response.ok, "{}", response.error.unwrap_or_default());
    response.data.context("control service returned no data")
}

fn registration(path: &str) -> anyhow::Result<Diagram> {
    let bytes = if path == "-" {
        std::io::read_to_string(std::io::stdin().lock())?
    } else {
        std::fs::read_to_string(path)?
    };
    let result: Value = serde_json::from_str(&bytes)?;
    let result = result.get("result").unwrap_or(&result);
    serde_json::from_value(result.get("structuredContent").unwrap_or(result).clone())
        .context("expected an Arconis saved workspace with a title, workspace_id and room_id")
}

fn select(diagrams: &[Diagram], name: &str) -> anyhow::Result<Diagram> {
    let mut matches = diagrams
        .iter()
        .filter(|d| d.title == name || d.url() == name);
    let diagram = matches
        .next()
        .context("diagram not found; use sulion plan diagram list")?;
    ensure!(
        matches.next().is_none(),
        "multiple diagrams have that title; use its room URL"
    );
    Ok(diagram.clone())
}

fn phase_link(value: &str, diagram: &Diagram, plan: &Value) -> anyhow::Result<DiagramPhaseLink> {
    let (number, url) = value.split_once('=').context("expected PHASE=OBJECT_URL")?;
    let number = number.parse::<i32>()?;
    let phase = plan["phases"]
        .as_array()
        .context("missing phase list")?
        .iter()
        .find(|phase| phase["position"].as_i64() == Some(i64::from(number)))
        .ok_or_else(|| anyhow!("phase {number} not found"))?;
    let object_id = url
        .strip_prefix(&format!("{}&item=", diagram.url()))
        .context("expected an object link from this Arconis room")?;
    Ok(DiagramPhaseLink {
        phase_id: serde_json::from_value(phase["id"].clone())?,
        object_id: object_id.to_owned(),
    })
}

pub(super) async fn run(mut args: Vec<String>, json: bool) -> anyhow::Result<i32> {
    match args.as_slice() {
        [action] if action == "list" => {
            let result = data(ControlRequest::PlanDiagramList).await?;
            reminder::print_attached(json).await;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                for diagram in serde_json::from_value::<Vec<Diagram>>(result["diagrams"].clone())? {
                    println!("{}  {}", diagram.title, diagram.url());
                }
            }
            return Ok(0);
        }
        [action, path] if action == "register" => {
            let result = data(ControlRequest::PlanDiagramRegister {
                diagram: registration(path)?,
            })
            .await?;
            reminder::print_attached(json).await;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                let diagram: Diagram = serde_json::from_value(result["diagram"].clone())?;
                println!("Registered {}  {}", diagram.title, diagram.url());
            }
            return Ok(0);
        }
        [action, ..] if action == "attach" || action == "detach" => {}
        _ => {
            bail!("expected diagram list, register, attach or detach; see sulion plan help diagram")
        }
    }
    let attach = args.remove(0) == "attach";
    let plan_id = take_uuid_option(&mut args, "--plan")?;
    let mut links = Vec::new();
    if attach {
        while let Some(link) = take_option(&mut args, "--link")? {
            links.push(link);
        }
    }
    let (name, remaining) = args
        .split_first()
        .context("diagram title or room URL required")?;
    reject_unknown_options(remaining)?;
    let registry = data(ControlRequest::PlanDiagramList).await?;
    let diagrams: Vec<Diagram> = serde_json::from_value(registry["diagrams"].clone())?;
    let diagram = select(&diagrams, name)?;
    let plan = data(ControlRequest::PlanShow { plan_id }).await?;
    let workspace_id = diagram.workspace_id;
    let operation = if attach {
        let phase_links = links
            .iter()
            .map(|link| phase_link(link, &diagram, &plan))
            .collect::<anyhow::Result<_>>()?;
        PlanEdit::AttachDiagram {
            workspace_id,
            phase_links,
        }
    } else {
        PlanEdit::DetachDiagram { workspace_id }
    };
    // Pin both the plan and its authored state while translating displayed phase numbers.
    let request = ControlRequest::PlanEdit {
        plan_id: Some(serde_json::from_value(plan["id"].clone())?),
        input: Box::new(DirectEdit {
            operation,
            reason: None,
            revision: plan["authoring"]["revision"].as_i64(),
        }),
    };
    reminder::run(request, json, false).await
}
