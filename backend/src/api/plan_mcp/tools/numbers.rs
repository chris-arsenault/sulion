use rmcp::model::JsonObject;
use schemars::JsonSchema;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    db::Pool,
    plans::{self, PhaseReference, PlanView},
};

#[derive(JsonSchema)]
#[allow(dead_code)]
pub(super) struct NumberedPhase {
    phase: PhaseReference,
    title: Option<String>,
    description: Option<String>,
    implementation: Option<String>,
    acceptance: Option<String>,
    basis: Option<Vec<PhaseReference>>,
    depends_on: Option<Vec<PhaseReference>>,
    follows_up: Option<Vec<PhaseReference>>,
    size: Option<String>,
}

pub(super) async fn resolve_direct_numbers(
    pool: &Pool,
    id: Uuid,
    tool: &str,
    input: &mut JsonObject,
) -> anyhow::Result<()> {
    let phase_tool = tool.ends_with("_phase") || tool == "record_plan_evidence";
    let mut fields = Vec::new();
    if phase_tool || tool.ends_with("_entry") {
        for key in ["phase", "entry", "number"] {
            if let Some(value) = input.remove(key) {
                anyhow::ensure!(
                    input.insert("id".to_owned(), value).is_none(),
                    "supply one target"
                );
            }
        }
        for key in ["id", "after", "successors"] {
            fields.push((key, phase_tool));
        }
        for key in ["basis", "goal", "illustrates", "answer"] {
            fields.push((key, false));
        }
        for key in ["depends_on", "follows_up"] {
            fields.push((key, true));
        }
    }
    let evidence = tool == "record_plan_evidence" && !input.contains_key("observed_revision");
    let has_number = |v: &Value| {
        v.is_number()
            || v.as_array()
                .is_some_and(|list| list.iter().any(Value::is_number))
    };
    if !evidence
        && !fields
            .iter()
            .any(|(key, _)| input.get(*key).is_some_and(has_number))
    {
        return Ok(());
    }
    let plan = plans::get(pool, id).await?;
    let current = plan
        .authoring
        .as_ref()
        .expect("full plan includes authoring")
        .revision;
    if evidence {
        input.insert("observed_revision".into(), json!(current));
        let ancestors = plan
            .inherited_authoring
            .iter()
            .map(|a| (a.plan_id, a.revision))
            .collect::<std::collections::BTreeMap<_, _>>();
        input.entry("ancestors").or_insert(json!(ancestors));
    }
    for (key, phase) in fields {
        if let Some(value) = input.get_mut(key) {
            if let Some(list) = value.as_array_mut() {
                for item in list {
                    resolve(item, phase, &plan)?;
                }
            } else {
                resolve(value, phase, &plan)?;
            }
        }
    }
    Ok(())
}

fn resolve(value: &mut Value, phase: bool, plan: &PlanView) -> anyhow::Result<()> {
    if !value.is_number() {
        return Ok(());
    }
    let number = value
        .as_i64()
        .filter(|n| *n > 0)
        .ok_or_else(|| anyhow::anyhow!("expected a positive item number"))?;
    let id = if phase {
        plan.phases
            .iter()
            .find(|p| i64::from(p.position) == number)
            .map(|p| p.id)
    } else {
        plan.authoring
            .as_ref()
            .and_then(|a| {
                a.document
                    .entries
                    .iter()
                    .chain(plan.inherited_authoring.iter().flat_map(|a| &a.entries))
                    .filter(|e| !e.retired)
                    .nth((number - 1) as usize)
            })
            .map(|e| e.id)
    }
    .ok_or_else(|| {
        anyhow::anyhow!(
            "no {} at position {number}",
            if phase { "phase" } else { "entry" }
        )
    })?;
    *value = json!(id);
    Ok(())
}
