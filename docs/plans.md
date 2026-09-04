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
- an optional parent plan plus the parent phases it covers, giving plan trees
  of arbitrary depth

Plan statuses are `active`, `paused`, `completed`, and `canceled`. Phase
statuses are `pending`, `in_progress`, `blocked`, `completed`, and `skipped`.
Closing a plan detaches all PTYs. Plans survive terminal exit and remain in the
repo's closed-plan history.

## Branch plans

A plan can hang off phases of another plan, to arbitrary depth. Work that turns
out to need its own multi-step job — a blocker discovered mid-phase, or a
milestone that has to be expanded before it can be executed — becomes a
sub-plan instead of being wedged into the parent's phase list or tracked in a
side file.

Nesting is at the plan level, so a branch is an ordinary plan: it carries its
own title, status, revision, history, and PTY attachment, and every plan
command works on it unchanged. What a branch adds is a parent and a set of
anchor phases:

- `parent_plan_id` is set at creation and never changes. `root_plan_id` and
  `depth` are derived from the parent at the same moment, so hierarchy reads
  never need a recursive query and cycles cannot occur.
- `plan_branch_anchors` records which parent phases the branch covers. One row
  is the common case; several rows let a branch cover a span, such as parent
  steps 4 through 6. Every anchor must belong to the branch's parent — anchors
  never span two plans, which is what keeps the return path unambiguous.
- Branching moves the acting PTY onto the branch. Any anchor still `pending`
  becomes `in_progress`; anchors the agent already marked `in_progress` or
  `blocked` keep the status it chose.
- Closing a branch puts the PTY back on the parent. Completing one also clears
  any anchor left `blocked`, since the branch existed to resolve that blocker.
  Canceling leaves anchor statuses untouched.
- A plan cannot close while a branch under it is open, and trees are capped at
  depth 8 as a runaway guard rather than a product limit.

Flow metrics read leaf phases only. A phase with a branch beneath it is a
container whose span and weight already cover the branch's phases, so counting
both would double-count the same work; burndown rolls a whole tree into one
line per root, which is what makes scope discovered mid-flight visible as a
line that rises.

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

sulion plan branch "Unblock the checkpoint gate" \
  --from 4 \
  --summary "Prerequisite for phase 4" \
  --phase "Instrument the divergence probe" \
  --phase "Fix the gate"
sulion plan branch "Expand M2" --from 4 --from 5 --from 6 --phase "Step one"
sulion plan return --completed --note "Gate is green"
sulion plan return --canceled --note "Wrong approach"
sulion plan tree

sulion plan close --completed
sulion plan close --completed --skip-remaining
sulion plan close --canceled --note "Superseded"
sulion plan history
```

`branch` opens a sub-plan under the current plan and moves the PTY onto it.
`--from` takes a 1-based phase position or a phase UUID and repeats for a span;
omitting it anchors to the parent's current phase. `return` closes the branch
and puts the PTY back on the parent — it refuses on a root plan, so the verb
can never quietly close the plan you branched from. `plan show` and `plan
current` print a branch's trail back to the root and list any sub-plans under
the phase they cover; `plan tree` prints the whole tree indented by depth.

`step` is accepted as an alias for `phase`. Most commands operate on the
current PTY's attached plan; `--plan <uuid>` or an explicit plan id targets
another plan. `sulion plan attach <uuid>` moves the PTY from any previous plan
to that plan. Add `--json` for stable machine-readable output.

Phases carry an optional t-shirt size (`--size s|m|l`, or a third `|size`
segment in `--phase "Title|Description|m"`). Sizes weight the burndown and
throughput charts (s=1, m=2, l=3); unsized phases count as weight 1.

Errors follow the shared agent-CLI contract (same as `sulion-code`): failures
print `sulion plan: <message>` plus a `next: <command>` recovery hint on
stderr, with exit codes 64 (usage), 65 (control socket unreachable), and 66
(request refused by the service). `sulion plan help` / `sulion activity help`
carry the full JIT reference — commands, status vocabularies, rules, and a
starting sequence.

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

Agents may also name their terminal — no permission required:

```sh
sulion name "ingest batcher refactor"
sulion name show
sulion name clear
```

The agent name complements the user's label (it never overwrites it) and is
shown beside it in the sidebar and team overview, deliberately not in tab
headers.

## Browser surfaces

Each repo has a **Plans** subsection in the sidebar. Its plan tab supports
creation, metadata edits, phase status/notes, phase addition, PTY attachment,
closure, and history. The plan index nests branches under the plan they hang
off; a branch's detail leads with a clickable trail back to the root, shows
sub-plans beneath the phase they cover, and replaces Complete/Cancel with
Return/Abandon. Each phase carries a branch control that opens a sub-plan under
it.

The **Metrics** tab (`/api/metrics`) aggregates the portfolio: non-overlapping
input, cached-input, and output rollups; a reconciled 14-day series from
`agent_model_usage_daily`; per-model usage with standard-tier API list-price
estimates; git activity per repo with agent/human commit attribution via
`Co-Authored-By` trailers; churn hotspots from repeated file writes; and
plan-flow charts (cumulative flow, per-plan burndown, throughput, cycle time)
replayed from `plan_events`. Input includes cache writes but excludes cache
reads. The projection retains ordinary and one-hour cache writes separately so
their provider rates can be applied without inference. Unknown model prices
remain visibly unpriced.

The **Overview** tab presents repos as teams and every live PTY as an engineer
card, including shells or sessions without an open browser tab. Teams needing
attention sort first. Each card combines operational activity, attached
plan/current phase, latest timeline output, agent uptime, cumulative token
spend, average token rate, and context remaining when the agent reports enough
data to calculate it. Its live token summary uses the same input, cached-input,
and output categories as Metrics. Missing usage telemetry is shown as
unavailable rather than estimated. Open plans remain visible even when no PTY
is attached.

## Runtime boundary

The CLI sends typed requests over the existing PTY correlation Unix socket.
The socket and browser REST handlers call the same plan/activity services and
write Postgres. Neither path reads transcript JSONL. The app-state poll carries
open-plan summaries plus each session's operational activity and current-plan
projection. The ingester also normalizes Codex cumulative usage snapshots and
Claude per-response usage into `agent_session_usage`; standard input, cache
writes, cache reads, output, cumulative spend, and the latest context footprint
stay separate.
