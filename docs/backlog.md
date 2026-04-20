# Feature Backlog

Still-open ideas, pruned of items that already shipped. See [`changelog.md`](changelog.md) for what landed. File a ticket before picking one up.

## Active candidates

1. **Resume / fork-from-event.** Button on any timeline event spawns a new PTY with `claude --resume <uuid> --fork-session`. Session correlation already gives us the UUID. Today we handle orphaned-session resume; this extends it to "fork from any earlier event." Single clearest thing a GUI can do that the CLI can't.
2. **Diff-review queue.** Aggregate unapproved edits from the current session into one reviewable surface. Cursor Composer's version of this is heavily praised. Diff tabs exist; this is the batched-approval rollup on top.
3. **Keybind cheatsheet overlay (`?`).** Discoverability for the existing palette + context menus.
4. **Per-session env / cwd badge.** Disambiguates sessions at a glance when several are in the same repo.
5. **Session-event permalinks (`#event=<id>`).** Point-share a specific decision.
6. **TodoWrite progress widget.** Persistent pinned widget showing the latest TodoWrite state — "what's Claude's plan right now" without scrolling to find the latest TodoWrite event. Parsing already exists.
7. **Auto-scroll-lock when the user selects.** Small polish; prevents lost selections in the live pane.
8. **File-touched panel.** Collapsible panel listing every file touched in the current session with per-file edit counts. Click a file → timeline auto-filters via the existing file-path facet. Cross-sectional "what did it change in foo.ts" view. Needs design sketching.
9. **Minimap / scrubber gutter.** Thin vertical strip alongside the timeline showing turn boundaries, error density, and tool-type distribution as ticks. Click-to-jump. Probably overkill until sessions regularly exceed a few thousand events.

## Speculative / big bets

**A. Semantic timeline — collapse by inferred task.** Group prompt → tool calls → summary into collapsible Warp-style blocks. The feature that would make sulion feel categorically different. Big design lift but typed events make it tractable.

**B. Cross-session search.** "What did I ask Claude about file X last week, across any session?" Postgres already has the data. Turns sulion into a knowledge base over your own agent history.

**C. Browser approval gates via PreToolUse hooks.** Route Claude's pause-on-risk into a browser modal any LAN device can approve. Leverages existing hook system + multi-device mirror. Real safety win for walk-away use.

**D. Plugin / custom renderer API.** Tool renderers are already modular; expose as a plugin point so users render MCP tools or custom hooks without forking. Higher risk (API surface, sandboxing) but fits the architecture.

## Explicitly NOT recommended

1. **Live-pane AI autocomplete (Warp-style).** The PTY is an AI agent. Stacking another AI on the input line fights the model and doubles cost.
2. **"Replace tmux" — panes inside one PTY.** Conflicts with PTY-per-session design. Users who want that run tmux inside the PTY.
3. **SSH host browser.** Out of scope; container-local PTY is the design.
4. **Offline PWA / local sync.** Product is LAN-tethered by definition. Sync invites divergence bugs with no user gain.
5. **Vim / emacs modal keybinds in the timeline.** Timeline isn't a terminal; imposing modes on a virtualized DOM list is friction. The command palette solves discoverability without the mode-confusion tax.
