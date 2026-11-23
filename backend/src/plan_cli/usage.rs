//! Reference text for the agent-facing CLIs.

pub(super) fn help_topic(args: &[String]) -> Option<&[String]> {
    let is_help = |arg: &str| matches!(arg, "-h" | "--help");
    if args
        .first()
        .is_some_and(|arg| arg == "help" || is_help(arg))
    {
        return Some(&args[1..]);
    }
    let topic = command_topic(args);
    args.get(topic.len())
        .filter(|arg| is_help(arg))
        .map(|_| topic)
}

pub(super) fn command_topic(args: &[String]) -> &[String] {
    let depth = match args {
        [group, command, ..]
            if matches!(group.as_str(), "phase" | "step" | "entry")
                && !command.starts_with('-') =>
        {
            2
        }
        _ => 1,
    };
    &args[..depth.min(args.len())]
}

pub(super) fn print_plan_help(topic: &[String]) -> anyhow::Result<()> {
    let first = topic.first().map(String::as_str).unwrap_or_default();
    let second = topic.get(1).map(String::as_str).unwrap_or_default();
    let first = if first == "step" { "phase" } else { first };
    let (usage, example, detail) = match (first, second, topic.len()) {
        ("", "", 0) => {
            println!("Sulion plans for the current repository\n\n\
Usage: sulion plan [--json] <command> ...\n\n\
  Read:    current, list, show, review, tree, history\n\
  Write:   start, update, phase, entry, patch, status, close\n\
  Context: attach, detach, branch, return\n\n\
Use `sulion plan help <command>` for syntax and an example.\n\
Start with `sulion plan current`. Writes default to the attached plan.");
            return Ok(());
        }
        ("phase", "", 1) => {
            println!("Usage: sulion plan phase <add|set|edit|move|retire|evidence> ...\n\n\
Use a phase number from `plan current`, as displayed.\n\
  set   change status and add a progress note\n\
  edit  change the title, description, or implementation details\n\
Use `sulion plan help phase edit` for an example.");
            return Ok(());
        }
        ("entry", "", 1) => {
            println!("Usage: sulion plan entry <add|set|move|retire> ...\n\n\
Use `sulion plan help entry add` to see entry kinds and fields.");
            return Ok(());
        }
        ("start", "", 1) => (
            "start <title> [--summary <text>] --phase <title[|description[|size]]>... [--all-pending]",
            r#"start "Improve plan editing" --phase "Simplify the CLI" --phase "Verify the workflow""#,
            "Guidance flags: --outcome, --boundary <requirement|correction|proposal>|<text>, --principle, --assumption. Repeat list flags as needed. The first phase starts immediately unless --all-pending is supplied.",
        ),
        ("current", "", 1) => ("current", "current", "Read the attached plan with guidance, numbered phases and entries."),
        ("list", "", 1) => ("list [--all]", "list --all", "Include completed and canceled plans with --all."),
        ("show", "", 1) => ("show [title]", "show", "Read an explicit plan or the attached plan."),
        ("review", "", 1) => ("review [title] [--phase <number>]", "review --phase 2", "Read the plan with the design review checklist."),
        ("tree", "", 1) => ("tree [title]", "tree", "Show the plan's branch hierarchy."),
        ("history", "", 1) => ("history [title]", "history", "Show execution events and authored changes."),
        ("update", "", 1) => (
            "update [--title <text>] [--summary <text>] [--outcome <text>] [--technical-details <text>] [--note <text>]",
            r#"update --summary "Support public registration and owner review""#,
            "Use --plan to select another plan. Repeat --boundary, --principle, or --assumption to replace the supplied list. Use --clear-boundaries, --clear-principles, or --clear-assumptions to empty a list. See `plan help entry set` to change one entry.",
        ),
        ("patch", "", 1) => (
            "patch <edit> [-- <edit> ...] | patch -",
            r#"patch update --summary "Requirements in Git" -- phase edit 2 --acceptance "Review the diff""#,
            "Use ordinary update, entry and phase authoring commands. Separate edits with -- or supply one command per line on stdin. Quote multiword arguments on stdin. Numbers refer to the order at each edit. All edits commit together.",
        ),
        ("status", "", 1) => ("status <active|paused> [--note <text>]", r#"status paused --note "Waiting for a decision""#, "Use --plan to select another plan."),
        ("close", "", 1) => ("close <--completed|--canceled> [--skip-remaining] [--note <text>]", "close --completed", "Completing a plan with unfinished phases requires --skip-remaining."),
        ("branch", "", 1) => (
            "branch <title> --phase <title[|description[|size]]>... [--from <number>] [--all-pending]",
            r#"branch "Repair CI" --from 2 --phase "Verify the build""#,
            "Uses the in-progress phase by default. Repeat --from for multiple anchors. Accepts the guidance flags from `plan help start`, plus --note and --plan.",
        ),
        ("return", "", 1) => ("return [--completed|--canceled] [--skip-remaining] [--note <text>]", "return", "Complete the branch and reattach its parent. Use --canceled to abandon the branch."),
        ("attach", "", 1) => ("attach <title>", r#"attach "Improve plan editing""#, "Use a plan title from plan list."),
        ("detach", "", 1) => ("detach [title]", "detach", "Detach the selected or current plan from this terminal."),
        ("phase", "add", 2) => ("phase add <title> [--description <text>] [--status <status>] [--size <s|m|l>]", r#"phase add "Verify CI""#, "Use --plan to select another plan."),
        ("phase", "set" | "update", 2) => (
            "phase set <number> [status] [--note <text>]",
            r#"phase set 2 completed --note "CI passed""#,
            "Statuses: pending, in_progress, blocked, completed, skipped. Use --plan to select another plan. For authored fields see `plan help phase edit`.",
        ),
        ("phase", "edit", 2) => (
            "phase edit <number> [--title <text>] [--description <text>] [--implementation <text>] [--acceptance <text>]",
            r#"phase edit 5 --acceptance "Only approved trucks appear in public discovery""#,
            "Use a phase number from plan current. Edits apply to current state by default. Also accepts --plan, --reason, --size, --basis (entry numbers), --depends-on, and --follows-up. Lists are comma-separated.",
        ),
        ("phase", "move", 2) => ("phase move <number> [--after <number>]", "phase move 3 --after 1", "Omit --after to move to the beginning. Accepts --plan and --reason."),
        ("phase", "retire", 2) => ("phase retire <number> [--successors <phases>]", "phase retire 3 --successors 4,5", "Retire only unstarted phases without branch anchors. Accepts --plan and --reason."),
        ("phase", "evidence", 2) => (
            "phase evidence <number> <observation> [--reference <text>]",
            r#"phase evidence 2 "CI built both apps""#,
            "Record the observation against the current plan and ancestor context.",
        ),
        ("entry", "add", 2) => (
            "entry add <kind> <text> [entry fields]",
            r#"entry add boundary "Use the shared public identity provider" --attribution user_requirement"#,
            "Kinds: requirement, boundary, principle, assumption, decision, question, hypothesis, example. See `plan help entry set` for the fields.",
        ),
        ("entry", "set", 2) => (
            "entry set <number> [--text <text>] [--source <text>] [--rationale <text>]",
            r#"entry set 1 --text "Use shared public Auth0""#,
            "Kind-specific fields:\n  boundary: --attribution user_requirement|user_correction|agent_proposal\n  decision: --provisional true|false\n  question: --answer <number> or --reopen\n  hypothesis: --goal <number>, --test <text>, --result supported|contradicted|inconclusive|set_aside, --evidence <text>\n  example: --illustrates <number>, --generalization <text>\nAccepts --plan and --reason. Get entry numbers with `plan current`.",
        ),
        ("entry", "move", 2) => ("entry move <number> [--after <number>]", r#"entry move 2 --after 3"#, "Omit --after to move to the beginning. Accepts --plan and --reason."),
        ("entry", "retire", 2) => ("entry retire <number> [--successors <numbers>]", r#"entry retire 2 --successors 3"#, "Supply comma-separated successor numbers. Accepts --plan and --reason."),
        _ => anyhow::bail!("unknown help topic: {}", topic.join(" ")),
    };
    let example = format!("sulion plan {example}");
    println!("Usage: sulion plan {usage}\n\n{detail}\n\n  {example}");
    Ok(())
}

pub(super) fn print_name_usage() {
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
  keep it short (max 100 chars); name the task, not the model or session ID
  set it once the task is clear, before substantial work — no permission needed
  update it when the task changes; do this even when the user supplied a label
  navigation uses the user's name, then the agent's name, then the session ID

Start:
  sulion name \"ingest batcher refactor\""
    );
}

pub(super) fn print_archive_usage() {
    println!(
        "\
Sulion transcript archive — export idle sessions to object storage, purge
them down to their turn digest, restore them on request

Usage:
  sulion archive [--json] <command> ...

Commands:
  help
  status                       store, whether purging is enabled, last cycle and dump,
                               counts, recent requests
  run [--dry-run]              queue a cycle now (dry run only reports what is eligible)
  verify [--deep]              check every archived object against its session row;
                               --deep downloads and re-hashes each one
  restore --session <uuid>     bring one purged session back in full
  restore --month YYYY-MM      every purged session whose archive month matches
  restore --repo <name>        every purged session attributed to a repo
  restore --all                whole history, oldest month first
          [--purge-after]      purge again once the replay is verified
                               (always on for --all)
  list [--limit n]             recent requests and their results

Rules:
  requests are queued here and executed by the control process's archive
    loop; watch progress in the Jobs panel or with `list`
  a purged session keeps its turn digest: prompt, assistant text, tool
    headers, files touched, tokens. Search, `sulion-retrieve turn`, and
    file-history keep working on it; tool output and per-operation detail
    need a restore
  a restore replays the archived lines through the normal ingest path; the
    session then looks exactly as it did before the purge

Purging:
  off until the deployment sets SULION_ARCHIVE_PURGE_ENABLED to 1 in
    compose.yaml, which is a reviewed commit, not a command; until then a
    cycle exports and dumps and deletes nothing
  sulion archive verify --deep  re-reads every object from the store; run it
                                before that commit

Start:
  sulion archive status
  sulion archive restore --session <agent-session-uuid>"
    );
}

pub(super) fn print_activity_usage() {
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
