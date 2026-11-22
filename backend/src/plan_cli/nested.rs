use std::collections::BTreeMap;

use anyhow::{anyhow, bail};
use serde::de::DeserializeOwned;
use serde_json::json;
use uuid::Uuid;

use crate::plans::{DirectEdit, EntryContent, EntryFields, PhaseFields, PhaseReference, PlanEdit};

use super::{
    parse_uuid, reject_unknown_options, take_flag, take_option, take_positional, take_uuid_option,
    ControlRequest,
};

pub(super) fn revision(args: &mut Vec<String>) -> anyhow::Result<Option<i64>> {
    Ok(take_option(args, "--revision")?
        .map(|value| value.parse())
        .transpose()?)
}

pub(super) fn request(
    plan_id: Option<Uuid>,
    revision: Option<i64>,
    reason: Option<String>,
    operation: PlanEdit<PhaseReference>,
) -> ControlRequest {
    ControlRequest::PlanEdit {
        plan_id,
        input: Box::new(DirectEdit {
            revision,
            reason,
            operation,
        }),
    }
}

pub(super) fn entry(args: &mut Vec<String>) -> anyhow::Result<ControlRequest> {
    let command = take_positional(args).ok_or_else(|| anyhow!("entry command is required"))?;
    let plan_id = take_uuid_option(args, "--plan")?;
    let revision = revision(args)?;
    let reason = take_option(args, "--reason")?;
    let operation = match command.as_str() {
        "add" => {
            let fields = entry_fields(args)?;
            let kind = take_positional(args).ok_or_else(|| anyhow!("entry kind is required"))?;
            let text = take_positional(args)
                .or_else(|| fields.text.clone())
                .ok_or_else(|| anyhow!("entry text is required"))?;
            let baseline: EntryContent = serde_json::from_value(json!({
                "kind": kind, "text": text, "attribution": fields.attribution,
            }))?;
            PlanEdit::AddEntry {
                entry: fields.apply(&baseline)?,
            }
        }
        "set" => {
            let fields = entry_fields(args)?;
            PlanEdit::EditEntry {
                id: id(args, "entry ID")?,
                fields,
            }
        }
        "move" => PlanEdit::MoveEntry {
            after: take_uuid_option(args, "--after")?,
            id: id(args, "entry ID")?,
        },
        "retire" => PlanEdit::RetireEntry {
            successors: ids(args, "--successors")?.unwrap_or_default(),
            id: id(args, "entry ID")?,
        },
        other => bail!("unknown entry command: {other}"),
    };
    reject_unknown_options(args)?;
    Ok(request(plan_id, revision, reason, operation))
}

pub(super) fn phase(command: &str, args: &mut Vec<String>) -> anyhow::Result<ControlRequest> {
    let plan_id = take_uuid_option(args, "--plan")?;
    let revision = revision(args)?;
    let reason = take_option(args, "--reason")?;
    let operation = match command {
        "edit" => {
            let title = take_option(args, "--title")?;
            let description = take_option(args, "--description")?;
            let implementation = take_option(args, "--implementation")?;
            let acceptance = take_option(args, "--acceptance")?;
            let basis = ids(args, "--basis")?;
            let depends_on = phase_refs(args, "--depends-on")?;
            let follows_up = phase_refs(args, "--follows-up")?;
            let size = take_option(args, "--size")?;
            PlanEdit::EditPhase(PhaseFields {
                id: phase_ref(args)?,
                title,
                description,
                implementation,
                acceptance,
                basis,
                depends_on,
                follows_up,
                size,
            })
        }
        "move" => PlanEdit::MovePhase {
            after: phase_option(args, "--after")?,
            id: phase_ref(args)?,
        },
        "retire" => PlanEdit::RetirePhase {
            successors: phase_refs(args, "--successors")?.unwrap_or_default(),
            id: phase_ref(args)?,
        },
        "evidence" => phase_evidence(args)?,
        other => bail!("unknown phase command: {other}"),
    };
    reject_unknown_options(args)?;
    Ok(request(plan_id, revision, reason, operation))
}

fn phase_evidence(args: &mut Vec<String>) -> anyhow::Result<PlanEdit<PhaseReference>> {
    let reference = take_option(args, "--reference")?;
    let revision = take_option(args, "--observed-revision")?
        .ok_or_else(|| anyhow!("evidence requires its --observed-revision"))?
        .parse()?;
    let mut ancestors = BTreeMap::new();
    while let Some(value) = take_option(args, "--ancestor")? {
        let (id, revision) = value
            .split_once('=')
            .ok_or_else(|| anyhow!("use --ancestor <plan-id>=<authoring-revision>"))?;
        ancestors.insert(parse_uuid(id, "ancestor plan ID")?, revision.parse()?);
    }
    Ok(PlanEdit::RecordEvidence {
        id: phase_ref(args)?,
        observation: take_positional(args)
            .ok_or_else(|| anyhow!("evidence observation is required"))?,
        reference,
        revision,
        ancestors,
    })
}

fn entry_fields(args: &mut Vec<String>) -> anyhow::Result<EntryFields> {
    Ok(EntryFields {
        text: take_option(args, "--text")?,
        source: take_option(args, "--source")?,
        rationale: take_option(args, "--rationale")?,
        attribution: keyword(args, "--attribution")?,
        provisional: take_option(args, "--provisional")?
            .map(|value| value.parse())
            .transpose()?,
        answer: take_uuid_option(args, "--answer")?,
        reopen: take_flag(args, "--reopen"),
        goal: take_uuid_option(args, "--goal")?,
        test: take_option(args, "--test")?,
        result: keyword(args, "--result")?,
        evidence: take_option(args, "--evidence")?,
        illustrates: take_uuid_option(args, "--illustrates")?,
        generalization: take_option(args, "--generalization")?,
    })
}

fn keyword<T: DeserializeOwned>(args: &mut Vec<String>, flag: &str) -> anyhow::Result<Option<T>> {
    take_option(args, flag)?
        .map(|value| {
            serde_json::from_value(json!(value)).map_err(|error| anyhow!("{flag}: {error}"))
        })
        .transpose()
}

fn id(args: &mut Vec<String>, label: &str) -> anyhow::Result<Uuid> {
    let value = take_positional(args).ok_or_else(|| anyhow!("{label} is required"))?;
    parse_uuid(&value, label)
}

fn phase_ref(args: &mut Vec<String>) -> anyhow::Result<PhaseReference> {
    take_positional(args)
        .ok_or_else(|| anyhow!("phase UUID or number is required"))?
        .parse()
}

pub(super) fn phase_option(
    args: &mut Vec<String>,
    flag: &str,
) -> anyhow::Result<Option<PhaseReference>> {
    take_option(args, flag)?
        .map(|value| value.parse())
        .transpose()
}

fn phase_refs(args: &mut Vec<String>, flag: &str) -> anyhow::Result<Option<Vec<PhaseReference>>> {
    take_option(args, flag)?
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::parse)
                .collect()
        })
        .transpose()
}

fn ids(args: &mut Vec<String>, flag: &str) -> anyhow::Result<Option<Vec<Uuid>>> {
    take_option(args, flag)?
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(|id| parse_uuid(id, flag))
                .collect()
        })
        .transpose()
}
