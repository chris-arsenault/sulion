use anyhow::{anyhow, bail, ensure, Context};
use uuid::Uuid;

use super::{
    nested, parse_phase_request, parse_plan_update, reject_unknown_options, take_option,
    take_positional, take_uuid_option, ControlRequest,
};
use crate::plans::{PhaseReference, PlanEdit, PlanFields};

pub(crate) fn split_edits<'a>(
    lines: impl IntoIterator<Item = &'a str>,
) -> anyhow::Result<Vec<Vec<String>>> {
    lines
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(i, line)| {
            shell_words::split(line).with_context(|| format!("invalid quoting in edit {}", i + 1))
        })
        .collect()
}

pub(super) fn request(args: &mut Vec<String>) -> anyhow::Result<ControlRequest> {
    let plan_id = take_uuid_option(args, "--plan")?;
    let revision = nested::revision(args)?;
    if !args
        .first()
        .is_some_and(|arg| arg.trim_start().starts_with('{'))
    {
        let reason = take_option(args, "--reason")?;
        let commands = cli_edits(args)?;
        return Ok(ControlRequest::PlanCommands {
            plan_id,
            input: crate::plans::PlanCommands {
                commands,
                revision,
                reason,
            },
        });
    }
    let payload = take_positional(args)
        .ok_or_else(|| anyhow!("provide inline JSON or use plan patch - to read stdin"))?;
    let mut input: crate::plans::PlanPatch<Option<i64>> = serde_json::from_str(&payload)?;
    if revision.is_some() {
        anyhow::ensure!(
            input.revision.is_none(),
            "provide the revision in JSON or with --revision"
        );
        input.revision = revision;
    }
    reject_unknown_options(args)?;
    Ok(ControlRequest::PlanPatch { plan_id, input })
}

fn cli_edits(args: &[String]) -> anyhow::Result<Vec<Vec<String>>> {
    ensure!(
        !args.is_empty(),
        "supply plan edits separated by -- or use patch - for one edit per line"
    );
    if let [text] = args {
        split_edits(text.lines())
    } else {
        Ok(args.split(|arg| arg == "--").map(<[_]>::to_vec).collect())
    }
}

/// Parse only authored changes. Never execute shell text or dispatch a lifecycle command.
pub(crate) fn authored_command(
    mut args: Vec<String>,
    entries: &[Uuid],
    revision: i64,
) -> anyhow::Result<PlanEdit<PhaseReference>> {
    ensure!(
        !args
            .iter()
            .any(|v| matches!(v.as_str(), "--plan" | "--revision" | "--reason" | "--note")),
        "select the plan and reason once for the whole batch"
    );
    let command = take_positional(&mut args).ok_or_else(|| anyhow!("empty plan edit"))?;
    let request = match command.as_str() {
        "entry" => nested::entry(&mut args, Some(entries))?,
        "phase" | "step" if args.first().is_some_and(|v| v == "add") => {
            parse_phase_request(&mut args)?
        }
        "phase" | "step" => {
            let action = take_positional(&mut args)
                .ok_or_else(|| anyhow!("expected phase edit, add, move, retire or evidence"))?;
            if action == "evidence" && !args.iter().any(|v| v == "--observed-revision") {
                args.extend(["--observed-revision".into(), revision.to_string()]);
            }
            nested::phase(&action, &mut args, Some(entries))?
        }
        "update" => parse_plan_update(&mut args)?,
        _ => bail!("expected update, entry or phase in an authoring batch, got {command}"),
    };
    match request {
        ControlRequest::PlanEdit { input, .. } => Ok(input.operation),
        ControlRequest::PlanUpdate { input, .. } => {
            ensure!(
                input.principles.is_none()
                    && input.boundaries.is_none()
                    && input.assumptions.is_none(),
                "use entry edits for guidance in a batch"
            );
            Ok(PlanEdit::EditPlan(PlanFields {
                title: input.title,
                summary: input.summary,
                outcome: input.outcome,
                technical_details: None,
            }))
        }
        ControlRequest::PlanAddPhase { phase, .. } => {
            ensure!(
                phase.status.as_deref().is_none_or(|s| s == "pending"),
                "a new phase in an authoring batch starts pending"
            );
            Ok(PlanEdit::AddPhase {
                id: Uuid::new_v4(),
                title: phase.title,
                description: phase.description,
                size: phase.size,
            })
        }
        _ => bail!("expected an authored plan edit"),
    }
}
