# Sulion user guide

A guide to sessions, review, prompts, and supervision. Existing screenshots
illustrate the feature surfaces and may predate the current toolbar layout;
the instructions below describe the current controls. Screenshots are captured
against the real e2e stack (Rust backend + Postgres + seeded ingest) through
Playwright, then cropped to feature regions with Pillow. To
regenerate:

```sh
make screenshots
```

That runs `frontend/e2e/99-tour.spec.ts` with
`SULION_SCREENSHOT_TOUR=1`, writes full-viewport PNGs and a bounding-box
manifest into `docs/screenshots/raw/`, then runs
`scripts/crop_screenshots.py` to emit the cropped PNGs this guide
references.

---

## Sidebar — meta-repos, repos, sessions, files, library

The left rail is the navigation surface. Meta-repositories provide one optional
organization level above repos without moving any checkout. Their collection
sessions appear at the parent; each member repo keeps its published plans,
single-repo sessions, isolated workspaces, lightweight file tree, and Git
status. Ungrouped repos remain first-class. The **Library**
section at the bottom lists saved prompts and references. Just above the
command palette entry, the rail also exposes the **Secrets** manager tab.
Saved prompts can include `$name` placeholders; `$$` sends a literal
`$`. Clicking a saved prompt, or sending a queued future prompt, inserts
its text into the active session's visible input: the timeline prompt
box in timeline-only mode (and on mobile), the terminal otherwise.

![Sidebar](screenshots/01-sidebar.png)

Right-click a session for rename / pin / colour / open-timeline /
delete actions, including the shortcut to manage secrets for that PTY.
New agent sessions can target one repo or every member of a meta-repository.
Collection sessions use canonical checkouts only. The primary repo supplies the
cwd; Claude and Codex receive the other current member roots as additional
directories. No member worktrees are created.
Right-click a repo for repo-level actions (open plans, open
repo timeline, repo diff). Double-click a session name to rename in place.

## Command palette

`Cmd+K` / `Ctrl+K` opens the command palette. It jumps to meta-repositories,
repos, and sessions, and can open a collection-session form. It drives the same
navigation the sidebar does — handy
when the tree is tall.

![Command palette](screenshots/02-command-palette.png)

## Create and resume sessions

Use a repo's new-session action to launch Claude Code, Codex, Fugu, or a shell.
Fugu is a Codex profile launched through `codex-fugu`; it uses the same
transcript format and timeline support as Codex.

For a single repo, choose the canonical checkout (`main`) or an isolated Git
worktree. File and diff tabs opened for that session follow its workspace.
Collection sessions always use canonical checkouts; see
[meta-repositories](meta-repositories.md) for their scope.

A disconnected browser can reattach to a running terminal. A dead shell needs
the resume action to create a new PTY and resume the conversation; this moves
the current association instead of creating a second live owner of the history.
On the dedicated node, releases preserve running shells. **Upgrade toolset
(restarts shell)** in a live session's menu explicitly replaces that shell on
the current toolset, preserving its session identity and workspace. Resume the
agent separately afterward. Combined standalone deployments do not preserve
shells across backend replacement.

## Workspace — terminal and timeline panes

On desktop, session navigation opens paired terminal and timeline tabs. The
terminal attaches to the live server shell; the timeline reviews its ingested
transcript. Use Display settings from the Overview modal or command palette
to choose **Split**, **Terminal only**, or **Timeline only**. A hidden
terminal keeps its connection and scrollback, and the desktop peek action
temporarily shows the hidden projection.

Mobile uses a timeline-only pane and sidebar drawer. It does not open terminal
tabs or offer terminal peeking. File and reference tabs remain available, and
the desktop display preference is preserved.

![Overview](screenshots/01-workspace.png)

Tabs support file, diff, overview, metrics, and reference kinds alongside terminal
and timeline, plus the Secrets manager tab. Plans open in a modal. Drag a tab
header onto the other pane's drop zone to split the work area; the layout persists
across reloads. The tab context menu can also close stale terminal and
session-timeline tabs whose backing session is no longer associated.

| Shortcut | Action |
| --- | --- |
| Cmd/Ctrl-K | Open command palette |
| Cmd/Ctrl-M | Open Overview; its links also reach Metrics and Display |
| Cmd/Ctrl-Shift-D | Cycle desktop display modes |
| Cmd/Ctrl-Shift-E | Peek at the hidden desktop terminal or timeline |
| Cmd/Ctrl-Shift-B | Toggle the desktop sidebar |

## Timeline — turns, filters, detail

Events are grouped into **turns** (prompt → tool calls → summary). The
turn list selects the detail to read. **Timeline settings** above the prompt
input opens filters, text size, and **List / Grid / Hidden** turn navigation.
Grid mode adds a separate **Turn grid** flyout button. These settings apply to
every open timeline and persist across reloads; terminal text size is separate.

Filter chips hide speakers, operation categories, and bookkeeping traffic;
the **FILE** input selects turns that touched a path. **Follow latest** tracks
new work; selecting an older turn turns it off so reading stays put. Turn
detail shows available duration and token metrics alongside the content.

![Timeline turn](screenshots/03-timeline-turn.png)

Prompt and assistant text render as GitHub-flavoured markdown. TeX math
renders through KaTeX: `$$ … $$` on its own lines or the LaTeX `\[ … \]`
form for a display block, `$$ … $$` within a line or `\( … \)` for inline
math. Single-dollar `$x$` is deliberately not math, so shell variables and
prices in prose stay as written. A link to a repo-relative path, with an
optional `:line` or `#L12` suffix, opens that file in a Sulion file tab for the
session's repo and workspace; `http(s)` links open in a new browser tab.
Nothing in turn detail navigates the app itself.

**Copy turn as markdown** copies the prompt, assistant text, and one header
per tool call. Open tool detail for full inputs, results, and diffs; those are
not embedded in the copied digest.

## Timeline input and prompt recovery

Use the prompt bar to **Send** to an idle agent or **Steer** a running turn.
The interrupt button stops the current turn through the terminal's interrupt
input. Large text and clipboard images can be saved into the session workspace
with paste-as-file; the composer receives the resulting file reference.

During harness startup or a terminal question, the composer closes and explains
why. On desktop, open or peek at the terminal to resolve the dialog. **Type
anyway** allows an intentional override. Mobile has no terminal view, so a
terminal-only startup interaction needs a desktop attachment.

The **Submitted prompts** button lists recent timeline sends, including text
that never matched a transcript turn. Each submission is saved before delivery.
Use copy or retry to recover unmatched text, or dismiss the record. A matched
record means the prompt appeared in a transcript; it does not mean the agent
completed the requested work.

If the harness changes away from the session's expected model, Sulion records
the switch, interrupts its running turn, and opens a model-switch dialog.
**Continue** accepts the new model. **Dismiss** retains the previous expected
model; it does not switch the harness back. The dialog shows available
transcript evidence, which may not explain why the harness changed models.

## Library and future prompts

Save reusable prompts and assistant references from timeline context menus.
The Library lists them across sessions; prompt templates ask for `$name`
values before insertion. Library entries are stored in Postgres.

Future prompts are one-off follow-ups queued for the current agent conversation.
Open the session's future-prompt queue to edit, insert, or remove an entry.
In timeline-only mode, insertion fills the composer and still needs Send or
Steer; in split or terminal-only mode it targets the terminal input. A queue
entry marked sent is separate from the actual submission records above.

## Published plans

Each repo's **Plans** subsection shows active and paused plans with phase
progress and the current phase. Open the repo plan index to start a plan with
named phases, optionally attach it to a live terminal, or reopen closed history.
The plan detail workspace edits phase descriptions, statuses, status notes,
attachments, plan metadata, and closure. Its history records meaningful
transitions. A phase can open a branch plan for a prerequisite or expanded
milestone. Branches appear beneath their parent with a trail back to the root;
**Return** completes a branch and moves the terminal back, while **Abandon**
cancels it. A parent cannot close while it has an open branch.

Published plans are a lightweight progress interface, not a replacement for an
agent's detailed working plan. Agents can publish the same state from a Sulion
PTY with `sulion plan`; see [`plans.md`](plans.md).

## Overview — engineering teams

The **Overview** tab shows every live terminal, whether or not its terminal tab
is currently open. Meta-repositories appear as teams containing the current
member repositories' terminals and open plans; collection sessions appear once
in that team and are labeled as collection work. Repositories outside a
meta-repository remain their own teams. Teams needing attention appear first.
Each card combines the agent's operational state and short summary, attached
plan/current phase, latest report, model, uptime, cumulative token spend,
average token rate, and context remaining. Context is only shown when the
transcript reports both current usage and a context window; unavailable signals
are labeled honestly. Open plans remain visible even when no terminal is
attached.

## Metrics and background jobs

Open **Metrics** from Overview to inspect input, cached input, and output
tokens, daily usage, and model-attributed cost estimates. Input includes cache
writes and excludes cache reads. Prices are the displayed catalog's
standard-tier API rates applied to recorded usage, not subscription charges or
historical invoices. Unknown model prices leave totals visibly incomplete.

The same view shows Git activity, file-write hotspots, and plan flow. Branch
plans contribute leaf phases to their root's burndown without double-counting
the parent phase. **Background jobs** shows active work, progress, stalled
writers, and recent completed, failed, or interrupted jobs.

## Thinking fly-out

Extended-thinking blocks collapse to a single chip in the turn detail.
Clicking **View thinking** pops them into a pinned fly-out that stays
on screen while you read the rest of the turn.

![Thinking fly-out](screenshots/04-thinking-flyout.png)

## Tool hover card

Hovering a tool row in the turn detail previews the full input and
result without expanding the row — useful for skimming a long turn
without losing your place. Pinning holds the card open.

![Tool hover card](screenshots/05-tool-hover.png)

## File tab with traceability

Opening a file from the sidebar tree (or from a tool reference) opens
a **File tab**. Above the body, the **Related timeline turns** panel
lists every turn that touched this file across the current session,
with a direct jump-back into the timeline.

![File tab](screenshots/06-file-tab.png)

The trace rows carry enough metadata — tool kind, speaker, timestamp —
to pick the right turn without opening each one.

![File traceability rows](screenshots/06-file-trace.png)

## Diff tab

Right-click a dirty file in the tree → **Open diff** to review the
working-tree changes. Each file hunk has its own **stage** button.

![Diff tab](screenshots/07-diff-tab.png)

## Secrets manager

The **Secrets** tab is Sulion's credential-management surface. Secrets
are stored as env bundles such as `ANTHROPIC_API_KEY=...` or AWS
credential sets. Grants are made from a terminal/session context menu
with a TTL. A grant enables that credential bundle for the PTY; both
`with-cred` and the `aws` wrapper redeem the same PTY-scoped grant.

The tab supports secret metadata and explicit key/value pairs. Once a
secret is saved, the UI shows only env key names; blank values on update
keep the existing value. Right-click a session or terminal tab and use
**Secrets** to enable a bundle or revoke an active grant.

## Context menus

One consolidated menu layer drives right-click actions on sessions,
tree nodes, tab handles, timeline turns, and library entries.

![Session context menu](screenshots/08-context-menu.png)

## Rename, pin, colour

Sessions can be renamed (double-click or **Rename** in the menu),
pinned to the top of their repo, and tinted with a colour. Pinned
sessions show a sigil; coloured rows get a stripe of the chosen tone.

![Pinned and coloured session](screenshots/09-session-pinned.png)

## Stats strip

The bottom of the sidebar carries a compact stats strip — the development
node's memory and CPU, Postgres size, event counts, live PTY and agent
session counts. Memory and CPU describe the machine your terminals actually
run on, so they read as dashes while no node is connected. Clicking expands
it into a detail panel.

![Stats panel](screenshots/10-stats-strip.png)

## Child-agent logs

For Claude and Codex sessions, delegated work stays associated with its parent
turn. **View agent log** opens the available child transcript in a modal
without losing the parent selection. Concurrent children retain separate logs.

![Agent log modal](screenshots/11-codex-subagent.png)

---

For the features referenced here, see
[`CHANGELOG.md`](../CHANGELOG.md) for the shipped-feature history and
[`backlog.md`](backlog.md) for what's still on the roadmap.
