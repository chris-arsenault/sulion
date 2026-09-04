//! JIT reference text for the agent-facing CLIs. This is what an agent reads
//! before its first `plan`/`activity`/`name` call, so it carries the full
//! command surface, status vocabularies, and rules rather than a synopsis.

pub(super) fn print_plan_usage() {
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
  branch <title> [--from <id|position>]... --phase <title[|description[|size]]>...
                 [--summary text] [--note text] [--all-pending]
  return [--completed|--canceled] [--skip-remaining] [--note text]
  tree [plan-id]
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

Branching:
  branch opens a sub-plan under the current plan and moves this PTY onto it
  --from names the parent phases the branch covers; repeat it for a span
    (--from 4 --from 5 --from 6). Omit it to anchor to the current phase
  return closes the branch and puts this PTY back on the parent, clearing any
    anchor phase the branch was blocked on. It refuses on a root plan
  branches nest to depth 8; a parent cannot close while a branch is open

Start:
  sulion plan current
  sulion plan start \"<title>\" --summary \"<text>\" --phase \"Title|Description\"
  sulion plan phase set 1 completed --note \"...\"
  sulion plan branch \"Unblock X\" --from 4 --phase \"Diagnose\" --phase \"Fix\"
  sulion plan return --completed --note \"...\"
  sulion plan close --completed"
    );
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
  keep it short (max 100 chars); shown in the sidebar and team overview,
    never in tab headers
  set it when it helps the user tell terminals apart — no permission needed

Start:
  sulion name \"ingest batcher refactor\""
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
