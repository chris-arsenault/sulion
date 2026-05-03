# sulion — Feature Overview

A visual tour of the major surfaces. Screenshots are captured against
the real e2e stack (Rust backend + Postgres + seeded ingest) through
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

## Sidebar — repos, sessions, files, library

The left rail is the navigation surface. Repos group their PTY sessions,
a lightweight file tree, and per-repo git staleness. The **Library**
section at the bottom lists saved prompts and references. Just above the
command palette entry, the rail also exposes the **Secrets** manager tab.

![Sidebar](screenshots/01-sidebar.png)

Right-click a session for rename / pin / colour / open-timeline /
delete actions, including the shortcut to manage secrets for that PTY.
New agent sessions can run in an isolated Sulion Git worktree by
default, while explicit main-worktree sessions stay bound to the
canonical checkout. Right-click a repo for repo-level actions (new
session, open repo timeline, repo diff). Double-click a session name to
rename in place.

## Command palette

`Cmd+K` / `Ctrl+K` opens the command palette. It jumps to repos and
sessions, and drives the same navigation the sidebar does — handy
when the tree is tall.

![Command palette](screenshots/02-command-palette.png)

## Workspace — terminal and timeline panes

Each session opens with a terminal tab and a timeline tab. The terminal
pane is `xterm.js` mounted outside React — WebSocket bytes pipe
straight to it, so keystroke latency matches native SSH. The timeline
pane is a structured, virtualized review surface over the ingested
transcript.

![Overview](screenshots/01-workspace.png)

Tabs support file, diff, monitor, and reference kinds alongside terminal
and timeline, plus the Secrets manager tab. Drag a tab header onto the
other pane's drop zone to split the work area; the layout persists
across reloads.

## Timeline — turns, filters, detail

Events are grouped into **turns** (prompt → tool calls → summary). The
left column lists turns; clicking one opens its detail on the right.
Filter chips along the top hide speakers, operation categories, and
bookkeeping traffic; the **FILE** input narrows the timeline to turns
that touched a given path.

![Timeline turn](screenshots/03-timeline-turn.png)

When you are mostly reading the structured timeline, the prompt bar can
send text into the running Claude/Codex terminal without expanding the
full terminal pane.

## Monitor — active-session output

The **Monitor** tab shows the latest assistant output from active
sessions in one mixed view. It follows the same timeline-derived data
model as session timelines, but presents one card per active session so
you can scan several agents without jumping between tabs.

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
with a TTL for one of two execution paths:

- `with-cred`
- `aws`

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

The bottom of the sidebar carries a compact stats strip — backend
memory / CPU, Postgres size, event counts, live PTY and agent session
counts. Clicking expands it into a detail panel.

![Stats panel](screenshots/10-stats-strip.png)

## Codex subagent log

For Codex sessions the timeline surfaces delegated subagent turns with
a **View agent log** button that opens the full child log in a modal,
without losing your place in the parent turn.

![Agent log modal](screenshots/11-codex-subagent.png)

---

For the features referenced here, see
[`CHANGELOG.md`](../CHANGELOG.md) for the shipped-feature history and
[`backlog.md`](backlog.md) for what's still on the roadmap.
