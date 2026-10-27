# ADR 0014: Published Plans Are a Compact Projection; Branches Nest at the Plan Level

## Status

Accepted, 2026-10-22.

## Context

On 2026-07-23 the user asked for "a sulion native plan operation. light
weight and not replacing agent plans. more of an addendum/interface so that
agents can publish phases with short descriptions and status, and sulion can
show them in a repo", together with an overview of what every open terminal
is doing (session `019f90db-6638-7b22-b347-85cf79826eb0`, turn 47484).
Commit 3406df2 shipped that the next day.

By 2026-09-04 flat plans were not enough. The user described agents getting
stuck on a step and needing "an entire separate mini plan before coming back
to the main one", recursively, sometimes covering several parent steps
(session `042f9311-eaac-4946-a6ec-c14f9f37dac1`, turn 580). The evidence
from two other repositories showed two shapes: planned expansion, where the
`feature-start` and `plan-phase` skills expanded one milestone into steps in
a markdown file that Sulion could not see ("Sulion holds the label; the file
holds the substance"), and unplanned blockers found mid-phase.

By October agents were losing the user's scope limits and rejected
approaches across resumes and branches; the published plan carried outcome,
principles, and assumptions but not boundaries, and nothing repeated them
(plan `67f9b0c4-5b11-45c3-a0b6-2ee0189a920a`).

## Decision

A published plan is a repo-scoped title, summary, status, revision, ordered
phases with status, note, and optional size, PTY attachments, and an
append-only event history. It is the small user-facing projection. Agents
keep their native planning mechanism for reasoning, dependencies, and
implementation steps. A file-backed working document, where one is needed,
exists only while its plan is open; its durable content moves into a topic
doc, an ADR, or the backlog when the plan closes.
The `sulion plan` CLI and the browser call the same services over the PTY
correlation socket and REST; neither reads transcript JSONL.

Branches nest at the plan level. A branch is an ordinary plan with a
`parent_plan_id` fixed at creation and a set of anchor phases in that parent;
`root_plan_id` and `depth` are derived at the same moment, so hierarchy reads
need no recursion and cycles cannot form. Anchors may span several parent
phases but never two plans. `plan branch` moves the terminal onto the
sub-plan and marks pending anchors in progress; `plan return` closes it,
returns the terminal, and clears an anchor left blocked. A parent cannot
close while a branch is open; depth is capped at 8 as a runaway guard. Flow
metrics read leaf phases only and draw one burndown per tree.

Guidance carries outcome, principles, assumptions, and sourced boundaries
(`requirement`, `correction`, `proposal`). Every plan command repeats the
outcome and boundaries, root first, with the owning plan and revision;
`--json` keeps stdout stable and prints the reminder on stderr. Reminders
restore context at tool boundaries; they are not runtime enforcement, and the
service does not certify who authorized a boundary.

## Alternatives considered

- **Make the published plan the agent's full plan.** Rejected by the user at
  the origin: "this would NOT replace the detailed plans that agents need"
  (turn 47484). `docs/plans.md` records the projection boundary from 3406df2
  onward.
- **Wedge sub-work into the parent's phase list.** Named in 8fde98c as the
  shape being replaced. Mid-flight detours displaced the plan they belonged
  to and left scope discovered mid-flight invisible.
- **Track sub-work in side files.** The other replaced shape in 8fde98c. The
  2026-09-04 analysis found exactly this in two repositories: the expansion
  lived in markdown and Sulion showed only the compact tracker.
- **Nest at the phase level.** 8fde98c chose plan-level nesting so a branch
  carries its own title, status, revision, history, and PTY attachment, and
  every existing plan operation works on it unchanged.
- **One anchor phase per branch.** The user asked for spans ("sub plan covers
  main plan steps 4-6"); anchors are a set, constrained to one parent so the
  return path stays unambiguous.
- **Per-plan burndowns counting container and branch phases.** 8fde98c: a
  phase with a branch beneath it already covers that branch's phases, so
  counting both measured the same work twice, and a per-plan chart let
  detours displace the plan they belong to.
- **Approval workflows, runtime enforcement, or transcript interpretation for
  boundaries.** Excluded by the agreed boundary of plan `67f9b0c4`: extend the
  existing guidance storage, inheritance, and history; add no separate
  enforcement subsystem.

## Consequences

- A substantive feature may keep a working document beside its published
  plan while the plan is open. A closed plan leaves a decision record, not a
  file; the published plan's history remains the durable progress record.
- Parentage is immutable, so a branch cannot be re-parented; cycles are
  impossible without a recursive check.
- Every plan command now prints the outcome and inherited boundaries, which
  the user accepted as the cost of context restoration on resume.
- Recorded `sulion plan` invocations are recognized by the timeline projection
  and carry their own filter and metadata.
- Guidance limits bound the approach, not the requested coverage; an agent
  proposal recorded in a plan does not become a user requirement.

## References

- Published plans, branches, guidance: [`../plans.md`](../plans.md)
- Commits: 3406df2 (published plans and overview), db428c5 (phase sizes),
  8fde98c (branch plans; body records the rejected shapes), a927199
  (sourced boundaries and recurring reminders)
- Published plan: `67f9b0c4-5b11-45c3-a0b6-2ee0189a920a`
- `sulion plan help` (command surface, rules, branching)
- Transcript: `019f90db-6638-7b22-b347-85cf79826eb0` turn 47484,
  `042f9311-eaac-4946-a6ec-c14f9f37dac1` turn 580
