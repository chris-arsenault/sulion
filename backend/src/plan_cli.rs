use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{anyhow, bail};
use serde_json::Value;
use uuid::Uuid;

use crate::activity::ActivityState;
use crate::correlate::{ControlRequest, ControlResponse};
use crate::plans::{NewPhase, UpdatePhaseInput, UpdatePlanInput};

pub async fn run_plan(args: &[OsString]) -> anyhow::Result<i32> {
    let mut args = utf8_args(args, "plan")?;
    let json = take_flag(&mut args, "--json");
    let Some(command) = args.first().cloned() else {
        print_plan_usage();
        return Ok(64);
    };
    if matches!(command.as_str(), "help" | "-h" | "--help") {
        print_plan_usage();
        return Ok(0);
    }
    args.remove(0);
    let request = match parse_plan_request(&command, &mut args) {
        Ok(request) => request,
        Err(err) => return Ok(usage_failure(ResponseKind::Plan, &err)),
    };
    run_control(request, json, ResponseKind::Plan).await
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

fn parse_plan_request(command: &str, args: &mut Vec<String>) -> anyhow::Result<ControlRequest> {
    match command {
        "start" => {
            let title_option = take_option(args, "--title")?;
            let summary = take_option(args, "--summary")?.unwrap_or_default();
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
                phases,
                all_pending,
            })
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
        "update" => {
            let plan_id = take_uuid_option(args, "--plan")?;
            let title = take_option(args, "--title")?;
            let summary = take_option(args, "--summary")?;
            let note = take_option(args, "--note")?;
            reject_unknown_options(args)?;
            Ok(ControlRequest::PlanUpdate {
                plan_id,
                input: UpdatePlanInput {
                    title,
                    summary,
                    note,
                    ..Default::default()
                },
            })
        }
        "phase" | "step" => parse_phase_request(args),
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
                    title,
                    description,
                    status,
                    status_note,
                    position,
                    size,
                },
            })
        }
        other => bail!("unknown phase command: {other}"),
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
}

impl ResponseKind {
    fn tool(self) -> &'static str {
        match self {
            ResponseKind::Plan => "sulion plan",
            ResponseKind::Activity => "sulion activity",
            ResponseKind::Name => "sulion name",
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
        ResponseKind::Plan => print_plan_data(&data),
        ResponseKind::Activity => print_activity_data(&data),
        ResponseKind::Name => print_name_data(&data),
    }
    Ok(0)
}

fn print_name_data(data: &Value) {
    match data.get("agent_label").and_then(Value::as_str) {
        Some(name) => println!("{name}"),
        None => println!("No agent name set."),
    }
}

fn print_plan_data(data: &Value) {
    if let Some(items) = data.as_array() {
        if items.is_empty() {
            println!("No plans.");
            return;
        }
        if items[0].get("event_type").is_some() {
            for event in items {
                println!(
                    "{} {}{}",
                    event["created_at"].as_str().unwrap_or(""),
                    event["event_type"].as_str().unwrap_or("event"),
                    event["note"]
                        .as_str()
                        .map(|note| format!(" — {note}"))
                        .unwrap_or_default()
                );
            }
            return;
        }
        for item in items {
            print_plan_header(item);
        }
        return;
    }
    if data.get("event_type").is_some() {
        println!(
            "{}",
            serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string())
        );
        return;
    }
    print_plan_header(data);
    if let Some(phases) = data.get("phases").and_then(Value::as_array) {
        for phase in phases {
            println!(
                "  {}. [{}] {}{}",
                phase["position"].as_i64().unwrap_or_default(),
                phase["status"].as_str().unwrap_or("unknown"),
                phase["title"].as_str().unwrap_or(""),
                phase["status_note"]
                    .as_str()
                    .map(|note| format!(" — {note}"))
                    .unwrap_or_default()
            );
        }
    }
}

fn print_plan_header(plan: &Value) {
    println!(
        "{} [{}] {}",
        plan["id"].as_str().unwrap_or(""),
        plan["status"].as_str().unwrap_or("unknown"),
        plan["title"].as_str().unwrap_or("")
    );
    if let Some(summary) = plan["summary"].as_str().filter(|value| !value.is_empty()) {
        println!("  {summary}");
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

fn print_plan_usage() {
    println!(
        "\
Sulion published plans — durable, repo-scoped phase summaries

Usage:
  sulion plan [--json] <command> ...

Commands:
  help
  start <title> --summary <text> --phase <title[|description[|size]]>... [--all-pending]
  current
  list [--all]
  show [plan-id]
  update [--plan uuid] [--title text] [--summary text]
  status <active|paused> [--plan uuid] [--note text]
  close (--completed|--canceled) [--skip-remaining] [--note text]
  phase add <title> [--description text] [--status status] [--size s|m|l]
  phase set <id|position> <status> [--note text] [--position n] [--size s|m|l]
  attach <plan-uuid>
  detach [plan-uuid]
  history [plan-id]

Statuses:
  plan   active | paused (close sets completed or canceled)
  phase  pending | in_progress | blocked | completed | skipped
  size   optional t-shirt weight s | m | l for weighted burndown

Rules:
  repo and acting PTY are inferred from the current terminal
  a plan is the compact user-facing projection; keep detailed reasoning in
    your native plan tool
  start requires at least one --phase; the first begins in_progress unless
    --all-pending
  close --completed rejects unfinished phases unless --skip-remaining
  most commands target this PTY's current plan; --plan <uuid> overrides
  `step` is an alias for `phase`

Start:
  sulion plan current
  sulion plan start \"<title>\" --summary \"<text>\" --phase \"Title|Description\"
  sulion plan phase set 1 completed --note \"...\"
  sulion plan close --completed"
    );
}

fn print_name_usage() {
    println!(
        "\
Sulion terminal name — an agent-chosen name shown beside the user's label

Usage:
  sulion name [--json] <text> | show | clear

Commands:
  <text>   set this terminal's agent name (words join; quoting optional)
  show     print the current agent name
  clear    remove it

Rules:
  complements the user's label; never overwrites it
  keep it short (max 100 chars); shown in the sidebar and team overview,
    never in tab headers
  set it when it helps the user tell terminals apart — no permission needed

Start:
  sulion name \"ingest batcher refactor\""
    );
}

fn print_activity_usage() {
    println!(
        "\
Sulion terminal activity — what this terminal is doing right now

Usage:
  sulion activity [--json] <command>

Commands:
  help
  working [summary]
  waiting [reason]      (alias: needs-input)
  blocked [reason]
  awaiting [summary]    (alias: awaiting-prompt)
  status
  clear

Rules:
  routine working/idle transitions are reported automatically by the agent
    lifecycle; publish explicit states only when they say more
  use waiting only when a user decision or permission is actually required
  an explicit waiting/blocked state persists until an explicit working or
    clear releases it

Start:
  sulion activity status
  sulion activity waiting --reason \"Choose the migration policy\""
    );
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
}
