# ADR 0019: Display Modes and Tabs Rendered by Reference

## Status

Accepted, 2026-08-21. Extended 2026-08-25 (`8829f6f`): mobile is
timeline-only and does not write the desktop preference.

## Context

The work area was a fixed vertical split: terminal on top, timeline below.
The user asked for three desktop modes (split, terminal-only, timeline-only)
with a single reading pane in the two new modes, a hotkey to collapse the
sidebar, a hotkey to peek at the hidden projection in a modal, and a settings
surface on the overview and metrics modal.

Tab content was rendered inside whichever pane owned the tab. Moving a tab
between panes, or re-parenting it into an overlay, remounted its React
subtree. For a terminal tab that means a new WebSocket attachment and a lost
scrollback; architecture invariant 2 also requires the terminal pane to stay
outside React reconciliation on PTY bytes.

Mobile had a separate, older all-tabs rendering path that returned from
`WorkArea` before display-mode filtering ran, chose the top pane's active
tab (the terminal), and let display settings and hotkeys rewrite the
persisted desktop mode even though the mobile renderer ignored it. The user
reported that mobile "doesn't really work at all" and asked for it to be
timeline-only.

## Decision

Desktop offers split, terminal-only, and timeline-only display modes, stored
in `DisplayStore`, with Cmd/Ctrl-Shift-D to cycle, Cmd/Ctrl-Shift-B for the
sidebar, and Cmd/Ctrl-Shift-E to peek at the hidden projection. Single modes
collapse to one merged pane and hide the other projection's tabs.

Tab content renders once. A root-level `TabHost` renders each tab into a
stable, registry-owned DOM host; pane slots and the peek overlay claim that
host by reference through `TabSlot`. Pane drags, mode switches, and peeks
move live DOM rather than remounting, so terminals keep their WebSocket and
scrollback and the peek shows the same terminal instance the split view
does. Tab state dies when the tab closes, not when it moves or is hidden.

The merged layout has its own explicit selection, `activeSinglePaneId`, and
activates a tab through the pane that actually owns it; selecting a tab in a
single-pane layout never changes pane membership.

Mobile (below 768px) derives an effective timeline-only mode through
`displayPolicy.ts` without persisting a new mode. Session navigation on
mobile opens only the timeline; terminal tabs, terminal actions,
display-mode controls, and the peek are desktop-only. Library and
future-prompt injection targets the timeline composer in effective
timeline-only mode and the terminal otherwise.

## Alternatives considered

- **Remount tab content when a tab changes pane or is peeked.** The prior
  behavior. `0700d6c` replaced it because a remount drops the terminal's
  WebSocket and scrollback, and a peek would have shown a second terminal
  instance rather than the one in the split view.
- **A terminal tab on mobile, selected by the separate mobile renderer.** The
  shipped state before `8829f6f`. The mobile branch bypassed timeline-only
  filtering, so session navigation surfaced an invisible or unusable
  terminal, `TabHost` marked the timeline inactive and stopped its polling,
  and the mobile e2e test codified that behavior. Removed in favor of one
  single-pane rendering path shared with desktop.
- **Persisting a mobile display mode that overwrites the desktop
  preference.** Rejected: display settings and hotkeys on mobile changed the
  stored desktop mode while having no visible effect. The effective mode is
  derived per viewport, and sidebar views persist under separate desktop and
  mobile keys.
- **Writing a bottom-pane tab id into `activeByPane.top` for the merged
  pane.** The first single-pane implementation did this, violating pane
  ownership and losing the active tab on layout switch or reload. The
  explicit `activeSinglePaneId` field replaced it.

## Consequences

- Layout changes preserve live tab hosts; a fix in one projection path
  applies to desktop single modes and mobile alike.
- Every tab kind must render correctly inside a host it does not own; tab
  components keep their internal state local and never assume a parent
  pane.
- The peek overlay borrows the live host, so the hidden projection cannot be
  shown in two places at once.
- Mobile users cannot reach a terminal; the needs-input banner says so
  rather than offering a dead action. Terminal interaction requires a
  desktop viewport.
- Resizing from mobile back to desktop restores the stored desktop mode
  without remounting existing tabs.

## References

- Store rules and `TabHost`/`TabSlot`:
  [`../state-management.md`](../state-management.md)
- Display controls and mobile shape: [`../design.md`](../design.md),
  "Display and timeline controls"
- Frontend invariant 2: [`../architecture.md`](../architecture.md#invariants--do-not-break)
- Commits: `0700d6c` (display modes, tabs rendered by reference), `8829f6f`
  (mobile timeline-only), `177c465` (mobile timeline interaction fix)
- Transcript: session `900989d9-0915-428f-ad97-701295c4968a`, turn `2735793`
  (display modes request); session `01a035fc-7a81-78a3-80b0-eded2a0c4997`,
  turn `1454194` (mobile review and plan)
- CHANGELOG Unreleased, "Workspace and sessions"
