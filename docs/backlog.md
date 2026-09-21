# Feature Backlog

Open ideas and deferred maintenance, reviewed 2026-09-21. See
[`CHANGELOG.md`](../CHANGELOG.md) for shipped behavior and
[the plan index](plans/README.md) for implementation records.

## Active candidates

1. **Resume / fork-from-event.** Button on any timeline event spawns a new PTY with `claude --resume <uuid> --fork-session`. Session correlation already gives us the UUID. Today we handle orphaned-session resume; this extends it to "fork from any earlier event." Single clearest thing a GUI can do that the CLI can't.
2. **Diff-review queue.** Aggregate unapproved edits from the current session into one reviewable surface. Cursor Composer's version of this is heavily praised. Diff tabs exist; this is the batched-approval rollup on top.
3. **Keybind cheatsheet overlay (`?`).** Discoverability for the existing palette + context menus.
4. **Per-session env / cwd badge.** Disambiguates sessions at a glance when several are in the same repo.
5. **Session-event permalinks (`#event=<id>`).** Point-share a specific decision.
6. **TodoWrite progress widget.** Persistent pinned widget showing the latest TodoWrite state — "what's Claude's plan right now" without scrolling to find the latest TodoWrite event. Parsing already exists.
7. **Pause following on text selection.** Timeline selection of an older turn
   already disables follow-latest. Any additional pause triggered by selecting
   text should preserve that navigation behavior.
8. **File-touched panel.** Collapsible panel listing every file touched in the current session with per-file edit counts. Click a file → timeline auto-filters via the existing file-path facet. Cross-sectional "what did it change in foo.ts" view. Needs design sketching.
9. **Minimap / scrubber gutter.** Thin vertical strip alongside the timeline showing turn boundaries, error density, and tool-type distribution as ticks. Click-to-jump. Probably overkill until sessions regularly exceed a few thousand events.

## Speculative / big bets

**A. Group turns by inferred task.** Prompt/tool/assistant turn grouping already
exists. This proposal would group multiple turns into a higher-level task and
needs a separate interaction and inference design.

**B. Browser history search.** Cross-session lexical and semantic search already
ships through `sulion-retrieve`, including user text with `--include user`.
The remaining idea is a browser search surface with scope controls and jumps
into timeline detail.

**C. Browser approval gates via PreToolUse hooks.** Route Claude's pause-on-risk into a browser modal any LAN device can approve. Leverages existing hook system + multi-device mirror. Real safety win for walk-away use.

**D. Plugin / custom renderer API.** Tool renderers are already modular; expose as a plugin point so users render MCP tools or custom hooks without forking. Higher risk (API surface, sandboxing) but fits the architecture.

## Explicitly NOT recommended

1. **Live-pane AI autocomplete (Warp-style).** The PTY is an AI agent. Stacking another AI on the input line fights the model and doubles cost.
2. **"Replace tmux" — panes inside one PTY.** Conflicts with PTY-per-session design. Users who want that run tmux inside the PTY.
3. **SSH host browser.** Out of scope; container-local PTY is the design.
4. **Offline PWA / local sync.** Product is LAN-tethered by definition. Sync invites divergence bugs with no user gain.
5. **Vim / emacs modal keybinds in the timeline.** Timeline isn't a terminal; imposing modes on a virtualized DOM list is friction. The command palette solves discoverability without the mode-confusion tax.

## Deferred maintenance

- **Archive and retention.** [Database archive, backup, and monthly
  purge](plans/transcript-archive-and-purge.md) remains unimplemented. Preserve
  its user decisions and refresh the pre-September-20 sizing before starting.
- **Compatibility retirement.** The July cleanup stopped writing the old
  correlation/resume fields but retained readers for stale peers. Removing
  `claude_session_uuid` and its agent default, `claude_resume_uuid`, the
  `current_claude_session_uuid` column, or the unused `repos` table still
  requires evidence that no live peer writes or sends the old shape. See
  [cleanup Chunks 6 and 10](plans/archive/cleanup-and-hardening.md).
- **Host recovery retirement.** Keep `nix/repair-existing-install.md` until
  the dedicated host is confirmed off the retired `/etc/sulion` layout.
  Retire `sulion-stack-adopt` only after its documented old-container condition
  is satisfied. Source cleanup is not evidence about host state.
