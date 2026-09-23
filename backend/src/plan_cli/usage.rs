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

pub(super) fn print_archive_usage() {
    println!(
        "\
Sulion transcript archive — export idle sessions to object storage, purge
them down to their turn digest, restore them on request

Usage:
  sulion archive [--json] <command> ...

Commands:
  help
  status                       store, purge gate, last cycle and dump, counts, recent requests
  run [--dry-run]              queue a cycle now (dry run only reports what is eligible)
  verify [--deep]              check every archived object against its session row;
                               --deep downloads and re-hashes each one
  purge-gate on|off            allow or forbid deletion. Closed by default: a cycle
                               exports and dumps but purges nothing until opened
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

First run:
  sulion archive run            exports and dumps; the gate is closed, nothing is deleted
  sulion archive verify --deep  re-reads every object from the store
  sulion archive purge-gate on  only after verify reports 0 missing, 0 mismatched

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
