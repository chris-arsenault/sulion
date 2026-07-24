# Published plans and terminal activity

Sulion published plans are durable, repo-scoped progress summaries. They let an
agent publish a handful of named phases with short descriptions and statuses so
the same work remains visible across terminal tabs and over time.

They are deliberately not an agent's internal working plan. Agents should keep
using their native detailed planning mechanism for reasoning, dependencies, and
implementation steps. A published plan is the smaller user-facing projection.

## Data model

A plan has:

- a repo, title, short summary, status, revision, and audit timestamps
- ordered phases with a title, short description, status, and optional status
  note
- zero or more attached live PTYs; each PTY can have only one current plan
- an append-only event history for meaningful mutations

Plan statuses are `active`, `paused`, `completed`, and `canceled`. Phase
statuses are `pending`, `in_progress`, `blocked`, `completed`, and `skipped`.
Closing a plan detaches all PTYs. Plans survive terminal exit and remain in the
repo's closed-plan history.

## Agent CLI

The CLI is available inside a Sulion PTY and infers the repo and acting PTY:

```sh
sulion plan start "Native plans" \
  --summary "Publish durable progress" \
  --phase "Backend|Schema, service, API, and CLI" \
  --phase "Frontend|Plan workspace and overview" \
  --phase "Verify|Tests and documentation"

sulion plan current
sulion plan list
sulion plan show
sulion plan update --summary "Updated short description"
sulion plan status paused --note "Waiting for upstream"
sulion plan status active

sulion plan phase add "Polish" --description "Final interaction pass"
sulion plan phase set 1 completed --note "API and CLI complete"
sulion plan phase set 2 blocked --note "Needs a product decision"
sulion plan phase set 2 in_progress

sulion plan close --completed
sulion plan close --completed --skip-remaining
sulion plan close --canceled --note "Superseded"
sulion plan history
```

`step` is accepted as an alias for `phase`. Most commands operate on the
current PTY's attached plan; `--plan <uuid>` or an explicit plan id targets
another plan. `sulion plan attach <uuid>` moves the PTY from any previous plan
to that plan. Add `--json` for stable machine-readable output.

Creating a plan requires at least one phase. By default the first phase starts
`in_progress` and the rest start `pending`; `--all-pending` leaves all phases
pending. Completing a plan with unfinished phases is rejected unless
`--skip-remaining` is explicit.

## Terminal activity

Published plan progress and terminal activity are separate. Activity answers
what a live PTY is doing now:

- `shell` and `starting` are derived from the PTY/agent process
- `working`, `awaiting_prompt`, `needs_input`, `blocked`, and `unknown` are
  stored operational states

Claude hooks and Codex transcript lifecycle events report working/awaiting
states automatically where the agent exposes them. An agent can publish a more
specific state:

```sh
sulion activity working "Implementing plan API"
sulion activity waiting --reason "Choose the migration policy"
sulion activity blocked --reason "Required service is unavailable"
sulion activity status
sulion activity clear
```

Use `waiting` only when user action is actually required. An explicit
`needs_input` or `blocked` report is not overwritten by a later automatic
turn-complete signal; the next explicit working/clear transition releases it.

## Browser surfaces

Each repo has a **Plans** subsection in the sidebar. Its plan tab supports
creation, metadata edits, phase status/notes, phase addition, PTY attachment,
closure, and history.

The **Overview** tab presents repos as teams and every live PTY as an engineer
card, including shells or sessions without an open browser tab. Teams needing
attention sort first. Each card combines operational activity, attached
plan/current phase, latest timeline output, agent uptime, cumulative token
spend, average token rate, and context remaining when the agent reports enough
data to calculate it. Missing usage telemetry is shown as unavailable rather
than estimated. Open plans remain visible even when no PTY is attached.

## Runtime boundary

The CLI sends typed requests over the existing PTY correlation Unix socket.
The socket and browser REST handlers call the same plan/activity services and
write Postgres. Neither path reads transcript JSONL. The app-state poll carries
open-plan summaries plus each session's operational activity and current-plan
projection. The ingester also normalizes Codex cumulative usage snapshots and
Claude per-response usage into `agent_session_usage`; cumulative spend and the
latest context footprint stay separate.
