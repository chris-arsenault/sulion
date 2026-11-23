# Conversational plans and terminal activity

Sulion plans are durable, repo-scoped records for authoring and delivery. The
sidebar and Overview carry a compact synopsis and progress. The selected plan
also contains current requirements and decisions, open questions, hypotheses,
examples, technical design, and phase implementation and evidence. An agent can
resume the plan by title across terminal sessions and connected clients.

The main sidebar's Plans section shows a collapsible tree with each sub-plan
under its parent, plus progress and current-phase text. Plan titles wrap so
they remain readable at narrow widths. Selecting a row opens its detail.

The plan modal shows the same tree navigation beside the selected
plan on desktop. On mobile, selecting a plan opens its detail; Back to plans
returns to the same tree without discarding draft edits. Search retains each
match's ancestors and temporarily reveals collapsed branches. Show closed
includes completed and canceled plans; ancestors and the selected plan remain
visible for context. Arrow keys move through visible plans, Left/Right collapse
or expand branches, and Enter or Space opens the focused plan.

## Plan commands in the timeline

Recorded `sulion plan` shell invocations have a **plans** operation filter,
plan badges, and dedicated metadata in turn details and hover cards. Collapsed
rows show the action, title, phase, and requested status when those arguments
are literal. Expanded details also show supplied plan IDs, notes, guidance,
branch anchors, and phase definitions alongside the original command and result.
Requested status describes the command's input; an error result still means the
operation failed. Metadata never substitutes the plan's current state.

The projection parses Bash syntax and uses the plan CLI's argument grammar.
Quoted examples, comments, and function declarations do not become plan calls.
Shell expansions are not evaluated; dynamic arguments retain only literal
action information and the raw command. Claude Bash calls, Codex shell calls,
and literal shell calls inside Code Mode use the same projection. A cell that also performs other operations
keeps its original category and shows plan metadata in its details, so hiding
plans does not hide unrelated edits or commands.

On upgrade from timeline version 13, retained historical operation rows gain
the classification and metadata in place. Turn IDs, items, transcript offsets,
and unrelated operations stay intact; affected session caches are invalidated.
Purged sessions retain their frozen digests.

## Data model

A plan has:

- a repo, title, short summary, status, operational revision, authoring revision,
  and audit timestamps
- guidance: outcome text, an ordered list of principles, and an ordered list
  of assumptions, plus sourced boundaries; existing plans default to empty guidance
- ordered phases with a title, short description, status, and optional status
  note
- a full authored document with stable entry IDs and implementation details
  keyed by phase UUID
- zero or more attached live PTYs; each PTY can have only one current plan
- an append-only event history for meaningful mutations
- an optional parent plan plus the parent phases it covers, giving plan trees
  up to depth 8

Plan statuses are `active`, `paused`, `completed`, and `canceled`. Phase
statuses are `pending`, `in_progress`, `blocked`, `completed`, and `skipped`.
Closing a plan detaches all PTYs. Plans survive terminal exit and remain in the
repo's closed-plan history.

## Author across turns

Read the selected plan and its inherited entries before applying a change.
Preserve unrelated entries and delivery details. Edit only the fields the
turn changes. `show` and `current` display the native entry and phase IDs.
Editing wording keeps the ID. Sulion assigns IDs when adding entries and
phases. When a commitment
changes, retire its entry and name the successor IDs. Reopen a question by
clearing its answer and recording why. Evidence remains linked to the revisions
it actually checked, including any governing ancestor revisions.

A hypothesis records a testable idea in service of the goal, the expected
observation, and what the test found. Record acceptance of the overall outcome
independently. An example preserves the concrete case and its source. Ask what
role the named thing plays, what varies, and what broader behavior should hold.
Record that behavior within the user's scope and link its plan entry when one
exists. Keep both types distinct from commitments.

```sh
sulion plan current
sulion plan update --technical-details "Extend the existing refinement path."
sulion plan entry add example "An inn seeds nearby development" \
  --generalization "Site conditions and the template determine the seed"
sulion plan entry set 1 --rationale "Keep the source example with its interpretation"
sulion plan phase edit 2 --acceptance "The next round reads the updated fields"
sulion plan help phase edit
sulion plan patch update --summary "Requirements in Git" -- phase edit 2 --acceptance "Review the diff"
sulion plan history
sulion plan review
```

CLI edits use the attached plan unless you select one by title. For duplicate
titles, use the quoted list number, for example `sulion plan show '#2'`.
Use the phase and entry numbers from `plan current`. In a branch, inherited
entries follow local entries and can be referenced as the basis of a phase.
Use `phase set` for progress and `phase edit` for authored fields.

`plan patch` applies ordinary edit commands in sequence under one plan lock.
Separate commands with `--`, or use `plan patch -` with one shell-quoted edit per
line. The parser treats every argument as data. Entry and phase numbers refer to
the order when each edit runs. A failed edit rolls back the whole batch.
Existing clients can still submit the structured patch format. Terminal
attachment and status updates do not advance
the authoring revision. Changed execution state can still prevent a patch from
moving or retiring work. Retired pending phases stay in history with their
successors and carry no remaining workload.

Near handoff, use `sulion plan review [title] [--phase <number>]`. Trace each
affected phase through the existing entry point, owner, contracts, and
consumers. Research patterns by the actual design problem and record the
chosen idiom and its participants in Technical details. Keep implementation,
callers, and verification together when they establish one deliverable. After
revisions, read the synopsis alongside the resulting decisions and phases.

## Guidance during execution

Guidance preserves the reasons for the work alongside its phases:

- **Outcome:** what should improve for the user and what would demonstrate
  success.
- **Boundaries:** excluded scope, rejected approaches or substitutes, and
  conditions for reassessing the approach. Each entry identifies its source as
  a user requirement, user correction, or agent proposal. Include the relevant
  instruction or reference in the text when needed to preserve its meaning.
- **Principles:** concrete rules for choosing approaches, including constraints
  and their reasons. Keep user requirements distinct from implementation choices.
- **Assumptions:** beliefs the proposed approach depends on that new evidence
  might invalidate. Put uncertain interpretations here.

Keep guidance concise: outcome allows 1,000 characters; each list allows up to
10 nonempty entries of 500 characters. Agents can write guidance from the
request. Guidance does not authorize changes to user requirements or agreed
architectural boundaries.

At phase start, on resume, and when evidence contradicts the approach, read
`sulion plan current` and relevant source material. Check whether the next step
still advances the outcome within the boundaries and under the principles.
Revise steps when justified, explain consequential deviations, and ask before changing user requirements or
agreed architectural boundaries. Consult referenced source material rather than
treating guidance as a substitute for it.

Record user corrections as boundaries and revise the affected phases before
continuing. An agent proposal does not become a user requirement because it
appears in a plan. Do not weaken a user boundary to fit completed work. Boundaries
limit approaches without reducing requested coverage: "do not chase minor
optimizations" does not mean "stop after two improvements." Tests of a rejected
substitute cannot establish the requested outcome. Ordinary work inside the
agreed scope needs no additional approval.

`current` and `show` display guidance before phases. Branch reads include the
live guidance of every ancestor, root first, labeled with the source plan and
revision. Branch guidance adds local context; it does not replace or override
ancestor requirements. Guidance is read from its owner rather than copied into
children. The plan modal shows the same guidance and refreshes an open branch
when an open ancestor's revision changes.

Every plan command surfaces the outcome and boundaries, root first, with the
source plan, revision, and boundary source. Routine commands omit principles
and assumptions from the recurring reminder; `current` and `show` include the
full guidance. `list` and `help` use the attached plan. An explicit target uses
that plan, even when another plan is attached. `close`, `detach`, and `return`
retain the command target's context in their result; subsequent commands use
the new attachment. Clearing local boundaries does not clear inherited ones.

`--json` preserves the command's existing stdout shape and prints the recurring
reminder on stderr. In text mode, a successful mutation prints a short
confirmation alongside the reminder. Use `current` or `show` for the full plan.
Failed commands also show available context. Usage errors
show the attached plan; service refusals use the requested target. If context
is unavailable (including no attachment or an unreachable service), the CLI
says so without hiding the original error or reporting a successful operation
as failed. Help remains usable outside a managed PTY.

These reminders restore context at tool boundaries; they are not runtime
enforcement or proof that an agent has followed the instructions. Reading
`current` on resume remains necessary. Existing plans have no boundaries until
someone records them; the empty state does not waive existing user instructions.

```sh
sulion plan start "Plan guidance" \
  --outcome "Agents can recover product intent when resuming work." \
  --boundary "requirement|Keep outcome and boundaries visible on every plan command." \
  --boundary "proposal|Extend existing guidance without a separate enforcement system." \
  --principle "Preserve user requirements when adapting implementation steps." \
  --assumption "Agents consult the current plan at phase boundaries." \
  --phase "Implement" --phase "Verify"

sulion plan update --principle "Read guidance and source evidence before changing direction." \
  --note "Clarify how to judge a deviation"
sulion plan update --clear-assumptions --note "Assumption checked against execution"
sulion plan history
```

`start`, `branch`, and `update` accept `--outcome`, repeatable `--principle`,
`--assumption`, and `--boundary "source|text"`. Boundary sources are `requirement`,
`correction`, and `proposal`; the text may contain additional `|` characters.
An update replaces each supplied list in order; omitted fields stay unchanged.
Use an entry edit for a targeted change. Before replacing a whole list, read
the current plan and retain the boundaries still in effect.
`--clear-principles`, `--clear-assumptions`, and `--clear-boundaries` clear lists
and cannot be combined with entries for the same list. Use
`--outcome ""` to clear the outcome. The API uses `outcome`, `principles`, and
`assumptions` fields with empty strings/arrays for clearing. `boundaries` is an
ordered array of `{ "source": "user_requirement" | "user_correction" |
"agent_proposal", "text": "..." }` entries, with `[]` for clearing. Like the
other lists, it allows up to 10 entries with 500 characters of text each. Source
labels record attribution; the service does not infer or certify authorization.

Guidance changes increment the plan revision and append a `guidance_changed`
event in the same transaction, preserving actor, optional explanation, and
complete before/after values. CLI history and the browser history expose these
values. Initial nonempty guidance is recorded too; unchanged guidance does not
produce an extra event.

## Branch plans

A plan can hang off phases of another plan, up to depth 8. Work that turns
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
- Branching moves the acting PTY onto the branch. An execution branch starts
  pending anchors. Use `--all-pending` for a drafting expansion that leaves
  its anchors pending. Existing active and blocked anchors retain their status.
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
sulion plan phase edit 3 --title "Renamed phase" --description "Revised scope"
sulion plan phase move 3

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
`--from` takes a 1-based phase position and repeats for a span;
omitting it anchors to the parent's current phase. `return` closes the branch
and puts the PTY back on the parent — it refuses on a root plan, so the verb
can never quietly close the plan you branched from. `plan show` and `plan
current` print a branch's trail back to the root and list any sub-plans under
the phase they cover; `plan tree` prints the whole tree indented by depth.

`step` is accepted as an alias for `phase`. Most commands operate on the
current PTY's attached plan; `--plan "Plan title"` or a positional plan title targets
another plan. `sulion plan attach "Plan title"` moves the PTY from any previous plan
to that plan. Add `--json` for stable machine-readable output.

Phases carry an optional t-shirt size (`--size s|m|l`, or a third `|size`
segment in `--phase "Title|Description|m"`). Sizes weight the burndown and
throughput charts (s=1, m=2, l=3); unsized phases count as weight 1.

Errors follow the shared agent-CLI contract (same as `sulion-code`): failures
print `sulion plan: <message>` plus a `next: <command>` recovery hint on
stderr, with exit codes 64 (usage), 65 (control socket unreachable), and 66
(request refused by the service). Use `sulion plan help` for the command list
and `sulion plan help <command>` for syntax and an example.

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

Agents name their terminal once the task is clear, before substantial work,
and update it when the task changes. This applies to small tasks too and does
not require permission or a published plan:

```sh
sulion name "ingest batcher refactor"
sulion name show
sulion name clear
```

The agent name never overwrites the user's label. Navigation shows the user
name first, falls back to the agent name, and uses the session ID only when
neither name is set. When both names differ, the sidebar and team overview
show the user name as primary and agent name as secondary.

## Browser surfaces

Each repo has a **Plans** subsection in the sidebar. Its plan modal supports
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
projection. The ingester normalizes Claude per-response usage and Codex
response records (with a legacy cumulative-snapshot prefix) into
`agent_session_usage`; standard input, cache
writes, cache reads, output, cumulative spend, and the latest context footprint
stay separate.

## Plans MCP

The Streamable HTTP connector serves `/mcp`. It lists Sulion repositories,
reads plans and history, and creates unattached drafts with pending phases.
Select a plan by title, supplying its repository when needed. `read_plan` returns
numbered entries and phases. `update_plan_entry` accepts `entry` and
`update_plan_phase` accepts `phase`. For a batch, `patch_plan` accepts the same
editing commands as the terminal, for example:

```json
{"plan":"City design","edits":["update --summary 'Review city requirements'","phase edit 2 --acceptance 'Review the diff'"]}
```

All edits share one transaction and one history event. A terminal can resume
with `sulion plan attach "City design"`.

### Requirements and inventory files

Select a checkout with `repo` or `workspace_id`. A dataset is a UTF-8
`*.dataset.json` document containing its schema and its ordered items. The
development node reads the working copy on each call. Git stages and commits it
through the repository's ordinary workflow.

From a terminal, use `sulion dataset` in the product checkout. Paths are
relative to its Git root. Use `sulion dataset -C CHECKOUT ...` to select
another working copy. Both interfaces use the same document engine.

```bash
doc=docs/processes.dataset.json
sulion dataset list
sulion dataset define "$doc" --key process --key map \
  --field process=string --field map=string --field strategy=string \
  --allow map=World --allow map=Region --description 'strategy=Behavior at this scale'
sulion dataset schema "$doc"
sulion dataset set "$doc" process=water map=Region --row process=water map=World
sulion dataset read "$doc" --missing strategy --column map
sulion dataset set "$doc" process=water map=Region 'strategy=Follow the drainage network'
sulion dataset read "$doc" --where process=water --where map=Region
sulion dataset export "$doc" --format csv
sulion dataset import "$doc" docs/agent-updates.csv
```

On `set`, supply the full row key and just the fields to change. A bare field
name clears it. `field=` stores an empty string for a string field. Quote
arguments containing spaces, including names such as `'Process ID=runoff'`.
Arrays and objects accept JSON values. Each `--row` starts another item in
the same atomic edit. For a conditional edit, save `read --json` output and
pass that file to `set --if-unchanged`. The CLI checks the edited rows.

Read defaults to 25 rows. Export includes all matching rows unless you give it
`--limit`. Both accept `--where`, `--missing`, `--column`, `--offset`
and `--limit`. Export to a new checkout-relative file or stdout. Import a
checkout file or pipe data to `import "$doc" --format csv`. Use
`sulion dataset help COMMAND` for the full syntax.

For example, call `define_data_set` with:

```json
{
  "repo": "my-product",
  "path": "docs/processes.dataset.json",
  "schema": {
    "keys": ["process", "map"],
    "fields": {
      "process": {"type": "string"},
      "map": {"type": "string", "allowed": ["World", "Region"]},
      "strategy": {"type": "string", "description": "How the process behaves at this scale"}
    }
  }
}
```

Use the same repo and path with `fill_data_set_items`. Pass
`"items":[{"process":"water","map":"Region"}]` to enumerate a row.
Include `"strategy":"..."` in that row on a later call to fill one field.
Omission preserves a value and an explicit null clears it. Key fields are
mandatory. Other fields can remain incomplete. Fields support string, integer,
number, boolean, array and object values. Invalid batches leave the file intact,
and schema changes must accept the existing rows.

`list_data_sets` discovers documents. `read_data_set` accepts `where` for
exact matches, `missing` for gaps, `columns`, `offset` and `limit`. Set
`limit` to zero to read just the schema and matching count. A read also returns
row revisions in item order. A caller editing against that read can pass
`expected` with the key and revision for each row it needs to guard.
For a plan reference, record the repository, relative path and the same
`where` object. A terminal agent can use the same fields with `read --where`.

`import_data_set` merges rows into a defined schema. Give it a `format` of
`csv` or `jsonl` and either inline `data` or a checkout-relative
`source_path` from a file upload. CSV uses a header, blank cells for omitted
values, and JSON syntax for non-string cells. JSONL preserves explicit empty
strings and nested values. `export_data_set` returns the schema and rows in
either format. Use a new `destination_path` to write a large export directly
in the checkout. The canonical JSON file itself is a portable typed export.

`sulion dataset delete "$doc"` and MCP `delete_data_set` remove the named
`.dataset.json` file with its schema and all rows. On MCP, pass the checkout
and relative path. For a tracked document, Git shows the deletion in the
working tree for review and commit.

### File transfers and request size

In a terminal, `sulion upload SOURCE DESTINATION` installs a local file at
a checkout-relative path. For example, `sulion upload /tmp/processes.csv
docs/processes.csv` makes the file available to a subsequent dataset import.
Use `-C CHECKOUT` before the source to select a different checkout. The CLI
uses the same atomic destination installer and 50 MiB limit as MCP.

In ChatGPT, select the attachment for `upload_file` and specify a repository
or workspace, directory and destination name. The connector reads the
provider-issued HTTPS reference, computes the checksum, stages the bytes in
S3 and asks the owning node to install the verified object. Select the
attachment again if its download reference expires.
The tool advertises the `file` argument through `openai/fileParams`.
[ChatGPT file inputs](https://developers.openai.com/plugins/reference#file-apis)

Clients with raw file access can use `begin_file_upload` with the destination,
byte count and base64 SHA-256 digest. PUT the raw file to the returned grant
URL with the signed headers and the exact content length. Call
`complete_file_upload` with the returned id and original metadata. The grant
is bound to the authenticated user and destination. Both workflows share
the existing 50 MiB limit and replace an existing destination on success.

The public WAF size-label exception covers JSON POST requests to the exact
Sulion host and `/mcp` path. The frontend proxy and MCP transport enforce a
256 KiB request ceiling on the actual body. File bytes travel outside MCP.

The MCP route and exact OAuth discovery paths follow the existing ALB, reverse
proxy, and WireGuard route to the TrueNAS frontend. Like Tastebase, Sulion
serves `/.well-known/oauth-protected-resource` and
`/.well-known/oauth-authorization-server` on its own host. The latter points to
the shared Cognito login endpoints and advertises authorization-code flow with
S256 PKCE. Before sign-in, `/mcp` returns a `401` that points to resource discovery.

Terraform provisions the confidential `sulion-plan-mcp` app client in the
same Cognito pool as the Sulion browser app. The Komodo deployment supplies
the MCP client ID and Cognito domain through `secret-paths.yml`. Terraform
registers Claude's fixed callback and the exact ChatGPT callback in
`infrastructure/terraform/plan-mcp.tf`. Update that URL if ChatGPT assigns a
new one when the connection is recreated.

The backend checks the dedicated client ID, the configured Cognito owner,
and broker revocation. Resource-bound access tokens must name the MCP URL.
Audit events use the authenticated Cognito subject. MCP can work with plans
across Sulion repositories. In the web client, add `https://sulion.services.ahara.io/mcp`
with the dedicated OAuth client credentials, then sign in as the owner.

### Arconis diagrams

In a terminal, use `sulion plan diagram list` for registered titles and room
links. Register a saved Arconis workspace with `sulion plan diagram register
saved-workspace.json`, or pass its save/open result on stdin with `register -`.
Use `sulion plan diagram attach "City design"` on the current plan. To set
phase links, include one `--link "2=OBJECT_URL"` per diagram object, using its
Arconis deep link. Each attach replaces that diagram's links on the selected
plan. `sulion plan diagram detach "City design"` removes the attachment.

Enable the Arconis and Sulion Plan connectors in the same conversation. Ask
ChatGPT to use both with the plan and repository you name.

1. Use `read_plan` for phase IDs and URLs. Keep the authoring revision when
   replacing links based on that read.
2. Use `list_repo_arconis_diagrams` to find reusable diagrams. In Arconis,
   use `open_workspace` with the workspace UUID, or use
   `import_document` with `new_room: true` to create a diagram.
3. Add each phase URL as a link on its canvas object. Use Markdown links in the
   object's name when it references multiple phases. Use `save_workspace` to
   persist the canvas and return its workspace UUID, room ID, and title.
4. Use `register_arconis_diagram` in Sulion to add the saved workspace to the repo.
   Then call `attach_arconis_diagram` with the workspace UUID and the complete
   set of phase UUID and canvas object ID pairs for this plan.

The plan shows the diagram link and each phase shows its linked canvas objects.
The diagram's links open the corresponding phases directly. Detaching from one
plan leaves the workspace registered for other plans. Update the Arconis
backlinks in the same conversation when changing or removing phase links.
Arconis opens saved workspaces using the connected account's access.

See [ADR 0014](adrs/0014-published-plans-and-branch-plans.md) for plan trees and
[ADR 0031](adrs/0031-revisioned-plan-authoring.md) for the authored record and
its shared service boundary. [ADR 0032](adrs/0032-arconis-plan-references.md)
covers diagram ownership and cross-app navigation.
