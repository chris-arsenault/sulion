use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{anyhow, bail};
use serde_json::Value;
use uuid::Uuid;

use crate::activity::ActivityState;
use crate::correlate::{ControlRequest, ControlResponse};
use crate::plans::{
    BranchPlanInput, NewPhase, PlanEdit, PlanFields, PlanGuidance, UpdatePhaseInput,
    UpdatePlanInput,
};

mod archive;
mod batch;
pub(crate) use batch::{authored_command, split_edits};
mod display;
mod guidance;
mod nested;
mod reminder;
mod usage;
pub(crate) use display::print_plan_data;

pub use archive::run_archive;
use usage::{print_activity_usage, print_name_usage, print_plan_help};

pub async fn run_plan(args: &[OsString]) -> anyhow::Result<i32> {
    let mut args = utf8_args(args, "plan")?;
    let json = take_flag(&mut args, "--json");
    let Some(command) = args.first().cloned() else {
        print_plan_help(&[])?;
        reminder::print_attached(json).await;
        return Ok(64);
    };
    if let Some(topic) = usage::help_topic(&args) {
        let code = match print_plan_help(topic) {
            Ok(()) => 0,
            Err(err) => usage_failure(ResponseKind::Plan, &err),
        };
        reminder::print_attached(json || !topic.is_empty()).await;
        return Ok(code);
    }
    if command == "patch" && args.get(1).is_some_and(|flag| flag == "--schema") {
        if let Err(error) = reject_unknown_options(&args[2..]) {
            return Ok(plan_usage_failure("patch", &error).await);
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&schemars::schema_for!(
                crate::plans::PlanPatch<Option<i64>>
            ))?
        );
        reminder::print_attached(true).await;
        return Ok(0);
    }
    let topic = usage::command_topic(&args).join(" ");
    args.remove(0);
    if let Err(error) = resolve_plan_title(&command, &mut args).await {
        return Ok(plan_usage_failure(&topic, &error).await);
    }
    if command == "patch" && args.first().is_some_and(|arg| arg == "-") {
        // Drop the stdin lock before reporting any error through the socket.
        let payload = std::io::read_to_string(std::io::stdin().lock());
        match payload {
            Ok(payload) => args[0] = payload,
            Err(error) => return Ok(plan_usage_failure(&topic, &error.into()).await),
        }
    }
    let request = match parse_plan_request(&command, &mut args.clone()) {
        Ok(request) => request,
        Err(err) if err.is::<nested::NeedsPlan>() => {
            let plan_id = take_uuid_option(&mut args, "--plan")?;
            let revision = nested::revision(&mut args)?;
            let reason = take_option(&mut args, "--reason")?;
            args.insert(0, command.clone());
            ControlRequest::PlanCommands {
                plan_id,
                input: crate::plans::PlanCommands {
                    commands: vec![args],
                    reason,
                    revision,
                },
            }
        }
        Err(err) => return Ok(plan_usage_failure(&topic, &err).await),
    };
    let full = matches!(command.as_str(), "current" | "show");
    reminder::run(request, json, full).await
}

async fn resolve_plan_title(command: &str, args: &mut [String]) -> anyhow::Result<()> {
    let target = if let Some(index) = args.iter().position(|arg| arg == "--plan") {
        args.get_mut(index + 1)
    } else if matches!(
        command,
        "attach" | "detach" | "show" | "review" | "tree" | "history"
    ) {
        let index = (0..args.len())
            .find(|&i| !args[i].starts_with('-') && (i == 0 || args[i - 1] != "--phase"));
        index.and_then(|i| args.get_mut(i))
    } else {
        None
    };
    if let Some(title) = target {
        if Uuid::parse_str(title).is_err() {
            let result = call(ControlRequest::PlanLookup {
                title: title.clone(),
            })
            .await?;
            anyhow::ensure!(result.ok, "{}", result.error.unwrap_or_default());
            *title = result
                .data
                .unwrap_or_default()
                .as_str()
                .ok_or_else(|| anyhow!("missing plan in listing"))?
                .to_owned();
        }
    }
    Ok(())
}

async fn plan_usage_failure(topic: &str, error: &anyhow::Error) -> i32 {
    eprintln!("sulion plan: {error}\nnext: sulion plan help {topic}");
    reminder::print_attached(true).await;
    64
}

pub async fn run_activity(args: &[OsString]) -> anyhow::Result<i32> {
    let mut args = utf8_args(args, "activity")?;
    let json = take_flag(&mut args, "--json");
    let Some(command) = args.first().cloned() else {
        print_activity_usage();
        return Ok(64);
    };
    if matches!(command.as_str(), "help" | "-h" | "--help") {
        print_activity_usage();
        return Ok(0);
    }
    args.remove(0);
    let request = match parse_activity_request(&command, &mut args) {
        Ok(request) => request,
        Err(err) => return Ok(usage_failure(ResponseKind::Activity, &err)),
    };
    run_control(request, json, ResponseKind::Activity).await
}

pub async fn run_name(args: &[OsString]) -> anyhow::Result<i32> {
    let mut args = utf8_args(args, "name")?;
    let json = take_flag(&mut args, "--json");
    if args.is_empty() {
        print_name_usage();
        return Ok(64);
    }
    if matches!(args[0].as_str(), "help" | "-h" | "--help") {
        print_name_usage();
        return Ok(0);
    }
    let request = match parse_name_request(&mut args) {
        Ok(request) => request,
        Err(err) => return Ok(usage_failure(ResponseKind::Name, &err)),
    };
    run_control(request, json, ResponseKind::Name).await
}

fn parse_name_request(args: &mut Vec<String>) -> anyhow::Result<ControlRequest> {
    match args[0].as_str() {
        "show" => {
            args.remove(0);
            reject_unknown_options(args)?;
            Ok(ControlRequest::SessionNameGet)
        }
        "clear" => {
            args.remove(0);
            reject_unknown_options(args)?;
            Ok(ControlRequest::SessionNameSet { name: None })
        }
        // Everything else is the name itself; multiple words join so
        // quoting is optional.
        _ => Ok(ControlRequest::SessionNameSet {
            name: Some(args.join(" ")),
        }),
    }
}

fn parse_activity_request(command: &str, args: &mut Vec<String>) -> anyhow::Result<ControlRequest> {
    let request = match command {
        "working" => {
            let summary = take_option(args, "--summary")?.or_else(|| take_positional(args));
            ControlRequest::ActivitySet {
                state: ActivityState::Working,
                summary,
                reason: None,
            }
        }
        "waiting" | "needs-input" => {
            let summary = take_option(args, "--summary")?;
            let reason = take_option(args, "--reason")?.or_else(|| take_positional(args));
            ControlRequest::ActivitySet {
                state: ActivityState::NeedsInput,
                summary,
                reason,
            }
        }
        "blocked" => {
            let summary = take_option(args, "--summary")?;
            let reason = take_option(args, "--reason")?.or_else(|| take_positional(args));
            ControlRequest::ActivitySet {
                state: ActivityState::Blocked,
                summary,
                reason,
            }
        }
        "awaiting" | "awaiting-prompt" => {
            let summary = take_option(args, "--summary")?.or_else(|| take_positional(args));
            ControlRequest::ActivitySet {
                state: ActivityState::AwaitingPrompt,
                summary,
                reason: None,
            }
        }
        "clear" => ControlRequest::ActivityClear,
        "status" | "show" => ControlRequest::ActivityGet,
        other => bail!("unknown activity command: {other}"),
    };
    reject_unknown_options(args)?;
    Ok(request)
}

pub(crate) fn parse_plan_request(
    command: &str,
    args: &mut Vec<String>,
) -> anyhow::Result<ControlRequest> {
    match command {
        "start" => {
            let title_option = take_option(args, "--title")?;
            let summary = take_option(args, "--summary")?.unwrap_or_default();
            let guidance = guidance::parse_new(args)?;
            let all_pending = take_flag(args, "--all-pending");
            let mut phases = Vec::new();
            while let Some(raw) = take_option(args, "--phase")? {
                phases.push(parse_phase(&raw));
            }
            let title = title_option
                .or_else(|| take_positional(args))
                .ok_or_else(|| anyhow!("plan title is required"))?;
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanStart {
                title,
                summary,
                guidance,
                phases,
                all_pending,
            })
        }
        "branch" | "return" => parse_branch_request(command, args),
        "tree" => {
            let plan_id = optional_plan_id(args)?;
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanTree { plan_id })
        }
        "current" => {
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanCurrent)
        }
        "list" => {
            let include_closed = take_flag(args, "--all") || take_flag(args, "--include-closed");
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanList { include_closed })
        }
        "show" => {
            let plan_id = optional_plan_id(args)?;
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanShow { plan_id })
        }
        "review" => {
            let phase_id = nested::phase_option(args, "--phase")?;
            let plan_id = optional_plan_id(args)?;
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanReview { plan_id, phase_id })
        }
        "status" => {
            let plan_id = take_uuid_option(args, "--plan")?;
            let note = take_option(args, "--note")?;
            let status = take_positional(args).ok_or_else(|| anyhow!("plan status is required"))?;
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanUpdate {
                plan_id,
                input: UpdatePlanInput {
                    status: Some(status),
                    note,
                    ..Default::default()
                },
            })
        }
        "close" => {
            let plan_id = take_uuid_option(args, "--plan")?;
            let completed = take_flag(args, "--completed");
            let canceled = take_flag(args, "--canceled");
            if completed == canceled {
                bail!("close requires exactly one of --completed or --canceled");
            }
            let note = take_option(args, "--note")?;
            let skip_remaining = take_flag(args, "--skip-remaining");
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanUpdate {
                plan_id,
                input: UpdatePlanInput {
                    status: Some(if completed { "completed" } else { "canceled" }.to_string()),
                    note,
                    skip_remaining,
                    ..Default::default()
                },
            })
        }
        "update" => parse_plan_update(args),
        "patch" => batch::request(args),
        "phase" | "step" => parse_phase_request(args),
        "entry" => nested::entry(args, None),
        "attach" => {
            let raw = take_positional(args).ok_or_else(|| anyhow!("plan id is required"))?;
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanAttach {
                plan_id: parse_uuid(&raw, "plan id")?,
            })
        }
        "detach" => {
            let plan_id = optional_plan_id(args)?;
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanDetach { plan_id })
        }
        "history" => {
            let plan_id = optional_plan_id(args)?;
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanHistory { plan_id })
        }
        other => bail!("unknown plan command: {other}"),
    }
}

fn parse_plan_update(args: &mut Vec<String>) -> anyhow::Result<ControlRequest> {
    let plan_id = take_uuid_option(args, "--plan")?;
    let authoring_revision = nested::revision(args)?;
    let technical_details = take_option(args, "--technical-details")?;
    let input = UpdatePlanInput {
        authoring_revision,
        title: take_option(args, "--title")?,
        summary: take_option(args, "--summary")?,
        note: take_option(args, "--note")?,
        outcome: take_option(args, "--outcome")?,
        principles: guidance::parse_items(args, "--principle", "--clear-principles")?,
        assumptions: guidance::parse_items(args, "--assumption", "--clear-assumptions")?,
        boundaries: guidance::parse_boundaries(args)?,
        ..Default::default()
    };
    reject_unknown_options(args)?;
    if technical_details.is_some() {
        let replaces_lists =
            input.principles.is_some() || input.assumptions.is_some() || input.boundaries.is_some();
        anyhow::ensure!(!replaces_lists, "edit individual guidance entries or use plan patch to combine them with technical details");
        return Ok(nested::request(
            plan_id,
            authoring_revision,
            input.note,
            PlanEdit::EditPlan(PlanFields {
                title: input.title,
                summary: input.summary,
                outcome: input.outcome,
                technical_details,
            }),
        ));
    }
    Ok(ControlRequest::PlanUpdate { plan_id, input })
}

/// `branch` opens a sub-plan; `return` closes one and pops this PTY back to the
/// parent. `return` defaults to completed — abandoning a branch is the case
/// worth spelling out — and sets `require_branch` so it can never close a root.
fn parse_branch_request(command: &str, args: &mut Vec<String>) -> anyhow::Result<ControlRequest> {
    let plan_id = take_uuid_option(args, "--plan")?;
    if command == "return" {
        let canceled = take_flag(args, "--canceled");
        let completed = take_flag(args, "--completed");
        if completed && canceled {
            bail!("return takes at most one of --completed or --canceled");
        }
        let note = take_option(args, "--note")?;
        let skip_remaining = take_flag(args, "--skip-remaining");
        reject_unknown_options(args)?;
        return Ok(ControlRequest::PlanUpdate {
            plan_id,
            input: UpdatePlanInput {
                status: Some(if canceled { "canceled" } else { "completed" }.to_string()),
                note,
                skip_remaining,
                require_branch: true,
                ..Default::default()
            },
        });
    }
    let title_option = take_option(args, "--title")?;
    let summary = take_option(args, "--summary")?.unwrap_or_default();
    let guidance = guidance::parse_new(args)?;
    let note = take_option(args, "--note")?;
    let all_pending = take_flag(args, "--all-pending");
    let mut phases = Vec::new();
    while let Some(raw) = take_option(args, "--phase")? {
        phases.push(parse_phase(&raw));
    }
    let mut parent_phase_refs = Vec::new();
    while let Some(raw) = take_option(args, "--from")? {
        parent_phase_refs.push(raw);
    }
    let title = title_option
        .or_else(|| take_positional(args))
        .ok_or_else(|| anyhow!("branch title is required"))?;
    reject_unknown_options(args)?;
    Ok(ControlRequest::PlanBranch {
        plan_id,
        input: BranchPlanInput {
            title,
            summary,
            guidance,
            phases,
            all_pending,
            parent_phase_refs,
            note,
        },
    })
}

fn parse_phase_request(args: &mut Vec<String>) -> anyhow::Result<ControlRequest> {
    let command = take_positional(args).ok_or_else(|| anyhow!("phase command is required"))?;
    match command.as_str() {
        "add" => {
            let title_option = take_option(args, "--title")?;
            let description = take_option(args, "--description")?.unwrap_or_default();
            let status = take_option(args, "--status")?;
            let size = take_option(args, "--size")?;
            let plan_id = take_uuid_option(args, "--plan")?;
            let title = title_option
                .or_else(|| take_positional(args))
                .ok_or_else(|| anyhow!("phase title is required"))?;
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanAddPhase {
                plan_id,
                phase: NewPhase {
                    title,
                    description,
                    status,
                    size,
                },
            })
        }
        "set" | "update" => {
            let status_option = take_option(args, "--status")?;
            let plan_id = take_uuid_option(args, "--plan")?;
            let authoring_revision = take_option(args, "--revision")?
                .map(|value| value.parse::<i64>())
                .transpose()?;
            let status_note = take_option(args, "--note")?;
            let title = take_option(args, "--title")?;
            let description = take_option(args, "--description")?;
            let size = take_option(args, "--size")?;
            let position = take_option(args, "--position")?
                .map(|value| value.parse::<i32>())
                .transpose()
                .map_err(|_| anyhow!("position must be an integer"))?;
            let phase_reference =
                take_positional(args).ok_or_else(|| anyhow!("phase id or position is required"))?;
            let status = status_option.or_else(|| take_positional(args));
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanUpdatePhase {
                plan_id,
                phase_reference,
                input: UpdatePhaseInput {
                    authoring_revision,
                    title,
                    description,
                    status,
                    status_note,
                    position,
                    size,
                },
            })
        }
        other => nested::phase(other, args, None),
    }
}

async fn call(control: ControlRequest) -> anyhow::Result<ControlResponse> {
    let pty_id = std::env::var("SULION_PTY_ID")
        .map_err(|_| anyhow!("SULION_PTY_ID is required inside a Sulion PTY"))?;
    let pty_id = parse_uuid(&pty_id, "SULION_PTY_ID")?;
    let socket = std::env::var_os("SULION_CORRELATE_SOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/sulion/correlate.sock"));
    crate::correlate::send_control(&socket, pty_id, control)
        .await
        .map_err(Into::into)
}

#[derive(Clone, Copy)]
enum ResponseKind {
    Plan,
    Activity,
    Name,
    Archive,
}

impl ResponseKind {
    fn tool(self) -> &'static str {
        match self {
            ResponseKind::Plan => "sulion plan",
            ResponseKind::Activity => "sulion activity",
            ResponseKind::Name => "sulion name",
            ResponseKind::Archive => "sulion archive",
        }
    }

    /// The `next:` hint printed when the control service refuses a request
    /// — points at the read command that shows the state the refusal was
    /// about (e.g. "no current plan" → list what exists).
    fn recovery(self) -> &'static str {
        match self {
            ResponseKind::Plan => "sulion plan list --all",
            ResponseKind::Activity => "sulion activity status",
            ResponseKind::Name => "sulion name show",
            ResponseKind::Archive => "sulion archive status",
        }
    }
}

// Exit codes mirror the sulion-code CLI contract: 64 usage error, 65 the
// control socket is unreachable (bad environment), 66 the request reached
// the service and was refused. Every failure prints a `next:` line.
fn usage_failure(kind: ResponseKind, err: &anyhow::Error) -> i32 {
    let tool = kind.tool();
    eprintln!("{tool}: {err}");
    eprintln!("next: {tool} help");
    64
}

async fn run_control(
    request: ControlRequest,
    json: bool,
    kind: ResponseKind,
) -> anyhow::Result<i32> {
    let tool = kind.tool();
    let response = match call(request).await {
        Ok(response) => response,
        Err(err) => {
            eprintln!("{tool}: {err}");
            eprintln!("next: {tool} help");
            return Ok(65);
        }
    };
    if !response.ok {
        let message = response
            .error
            .unwrap_or_else(|| "control request failed".to_string());
        eprintln!("{tool}: {message}");
        eprintln!("next: {}", kind.recovery());
        return Ok(66);
    }
    let data = response.data.unwrap_or(Value::Null);
    if json {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(0);
    }
    match kind {
        ResponseKind::Plan => print_plan_data(&data, true),
        ResponseKind::Activity => print_activity_data(&data),
        ResponseKind::Name => print_name_data(&data),
        ResponseKind::Archive => archive::print_archive_data(&data),
    }
    Ok(0)
}

fn print_name_data(data: &Value) {
    match data.get("agent_label").and_then(Value::as_str) {
        Some(name) => println!("{name}"),
        None => println!("No agent name set."),
    }
}

fn print_activity_data(data: &Value) {
    if data.is_null() {
        println!("No explicit activity state.");
        return;
    }
    if data["cleared"].as_bool() == Some(true) {
        println!("Activity state cleared.");
        return;
    }
    println!(
        "{}{}{}",
        data["state"].as_str().unwrap_or("unknown"),
        data["summary"]
            .as_str()
            .map(|value| format!(" — {value}"))
            .unwrap_or_default(),
        data["reason"]
            .as_str()
            .map(|value| format!(" ({value})"))
            .unwrap_or_default()
    );
}

/// `--phase "Title|Description|size"` — description and size segments are
/// optional; size is the t-shirt weight (s|m|l) for weighted burndown.
fn parse_phase(raw: &str) -> NewPhase {
    let mut parts = raw.splitn(3, '|').map(str::trim);
    let title = parts.next().unwrap_or("");
    let description = parts.next().unwrap_or("");
    let size = parts.next().filter(|value| !value.is_empty());
    NewPhase {
        title: title.to_string(),
        description: description.to_string(),
        status: None,
        size: size.map(str::to_string),
    }
}

fn optional_plan_id(args: &mut Vec<String>) -> anyhow::Result<Option<Uuid>> {
    take_uuid_option(args, "--plan")?.map_or_else(
        || {
            take_positional(args)
                .map(|value| parse_uuid(&value, "plan id"))
                .transpose()
        },
        |id| Ok(Some(id)),
    )
}

fn take_uuid_option(args: &mut Vec<String>, name: &str) -> anyhow::Result<Option<Uuid>> {
    take_option(args, name)?
        .map(|value| parse_uuid(&value, name))
        .transpose()
}

fn parse_uuid(value: &str, label: &str) -> anyhow::Result<Uuid> {
    Uuid::parse_str(value).map_err(|_| anyhow!("{label} must be a UUID"))
}

fn utf8_args(args: &[OsString], command: &str) -> anyhow::Result<Vec<String>> {
    args.iter()
        .map(|arg| {
            arg.to_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow!("{command} arguments must be valid UTF-8"))
        })
        .collect()
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    if let Some(index) = args.iter().position(|arg| arg == name) {
        args.remove(index);
        true
    } else {
        false
    }
}

fn take_option(args: &mut Vec<String>, name: &str) -> anyhow::Result<Option<String>> {
    let Some(index) = args.iter().position(|arg| arg == name) else {
        return Ok(None);
    };
    if index + 1 >= args.len() || args[index + 1].starts_with("--") {
        bail!("{name} requires a value");
    }
    args.remove(index);
    Ok(Some(args.remove(index)))
}

fn take_positional(args: &mut Vec<String>) -> Option<String> {
    let index = args.iter().position(|arg| !arg.starts_with('-'))?;
    Some(args.remove(index))
}

fn reject_unknown_options(args: &[String]) -> anyhow::Result<()> {
    if let Some(value) = args.first() {
        bail!("unexpected argument: {value}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_accepts_repeatable_phases() {
        let mut args = vec![
            "Feature".to_string(),
            "--summary".to_string(),
            "overview".to_string(),
            "--phase".to_string(),
            "Schema|Persist plans".to_string(),
            "--phase".to_string(),
            "UI".to_string(),
        ];
        let request = parse_plan_request("start", &mut args).unwrap();
        let ControlRequest::PlanStart {
            title,
            phases,
            summary,
            ..
        } = request
        else {
            panic!("expected plan start");
        };
        assert_eq!(title, "Feature");
        assert_eq!(summary, "overview");
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].description, "Persist plans");
    }

    #[test]
    fn close_requires_one_outcome() {
        let mut args = vec![];
        assert!(parse_plan_request("close", &mut args).is_err());
        let mut args = vec!["--completed".into(), "--canceled".into()];
        assert!(parse_plan_request("close", &mut args).is_err());
    }

    #[test]
    fn start_does_not_treat_option_values_as_the_title() {
        let mut args = vec![
            "--summary".to_string(),
            "overview".to_string(),
            "--phase".to_string(),
            "Schema".to_string(),
        ];
        let error = parse_plan_request("start", &mut args).unwrap_err();
        assert!(error.to_string().contains("plan title is required"));
    }

    #[test]
    fn phase_specs_carry_optional_size() {
        let mut args = vec![
            "Feature".to_string(),
            "--phase".to_string(),
            "Schema|Persist plans|m".to_string(),
            "--phase".to_string(),
            "UI|Frontend".to_string(),
        ];
        let request = parse_plan_request("start", &mut args).unwrap();
        let ControlRequest::PlanStart { phases, .. } = request else {
            panic!("expected plan start");
        };
        assert_eq!(phases[0].size.as_deref(), Some("m"));
        assert_eq!(phases[1].size, None);

        let mut args = vec![
            "add".to_string(),
            "Polish".to_string(),
            "--size".to_string(),
            "l".to_string(),
        ];
        let request = parse_plan_request("phase", &mut args).unwrap();
        let ControlRequest::PlanAddPhase { phase, .. } = request else {
            panic!("expected phase add");
        };
        assert_eq!(phase.size.as_deref(), Some("l"));
    }

    #[test]
    fn name_command_parses_set_show_and_clear() {
        let mut args = vec![
            "ingest".to_string(),
            "batcher".to_string(),
            "refactor".to_string(),
        ];
        let request = parse_name_request(&mut args).unwrap();
        assert_eq!(
            request,
            ControlRequest::SessionNameSet {
                name: Some("ingest batcher refactor".to_string())
            }
        );

        let mut args = vec!["show".to_string()];
        assert_eq!(
            parse_name_request(&mut args).unwrap(),
            ControlRequest::SessionNameGet
        );

        let mut args = vec!["clear".to_string()];
        assert_eq!(
            parse_name_request(&mut args).unwrap(),
            ControlRequest::SessionNameSet { name: None }
        );
    }

    #[test]
    fn activity_waiting_takes_a_positional_reason() {
        let mut args = vec!["Choose the migration policy".to_string()];
        let request = parse_activity_request("waiting", &mut args).unwrap();
        let ControlRequest::ActivitySet { state, reason, .. } = request else {
            panic!("expected activity set");
        };
        assert_eq!(state, ActivityState::NeedsInput);
        assert_eq!(reason.as_deref(), Some("Choose the migration policy"));
    }

    #[test]
    fn activity_rejects_unknown_commands_and_stray_arguments() {
        let mut args = vec![];
        assert!(parse_activity_request("panic", &mut args).is_err());
        let mut args = vec!["done".to_string(), "--bogus".to_string()];
        assert!(parse_activity_request("working", &mut args).is_err());
    }

    #[test]
    fn phase_set_accepts_options_before_its_position() {
        let mut args = vec![
            "set".to_string(),
            "--note".to_string(),
            "done".to_string(),
            "--status".to_string(),
            "completed".to_string(),
            "2".to_string(),
        ];
        let request = parse_plan_request("phase", &mut args).unwrap();
        let ControlRequest::PlanUpdatePhase {
            phase_reference,
            input,
            ..
        } = request
        else {
            panic!("expected phase update");
        };
        assert_eq!(phase_reference, "2");
        assert_eq!(input.status.as_deref(), Some("completed"));
        assert_eq!(input.status_note.as_deref(), Some("done"));
    }

    #[test]
    fn phase_set_renames_without_changing_status() {
        let mut args = vec![
            "set".to_string(),
            "3".to_string(),
            "--title".to_string(),
            "Renamed".to_string(),
            "--description".to_string(),
            "Revised scope".to_string(),
        ];
        let request = parse_plan_request("phase", &mut args).unwrap();
        let ControlRequest::PlanUpdatePhase {
            phase_reference,
            input,
            ..
        } = request
        else {
            panic!("expected phase update");
        };
        assert_eq!(phase_reference, "3");
        assert_eq!(input.title.as_deref(), Some("Renamed"));
        assert_eq!(input.description.as_deref(), Some("Revised scope"));
        assert_eq!(input.status, None);
    }
}
