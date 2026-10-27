# Feature Backlog

Open ideas and deferred maintenance, reviewed 2026-10-27. See
[`CHANGELOG.md`](../CHANGELOG.md) for shipped behavior and
[the decision records](adrs/README.md) for choices already made.

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
10. **Browser e2e for write-heavy paths.** Add real-stack coverage for diff
    staging, repo creation, isolated-workspace session creation, published-plan
    and Overview flows, and timeline filter combinations. The prioritized list
    lives in [the e2e coverage plan](e2e-coverage-plan.md).

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
4. **Offline PWA / local sync.** Product is LAN-tethered by definition. Sync invites divergence bugs with no user gain. Home-screen installation ships without a service worker for this reason ([ADR 0030](adrs/0030-standalone-mobile-install-without-service-worker.md)).
5. **Vim / emacs modal keybinds in the timeline.** Timeline isn't a terminal; imposing modes on a virtualized DOM list is friction. The command palette solves discoverability without the mode-confusion tax.

## Deferred maintenance

- **Verify production usage-model coverage.** Inventory every model name in
  live and archived usage rows of the deployed database and add any rate the
  catalog lacks. The pricing changes shipped; the inventory needs a terminal
  whose broker grant injects the `SULION_DB_*` variables, which the last
  attempt did not have.
- **Compatibility retirement, phase B.** The old correlation and resume
  shapes are no longer written but their readers remain for peers that can
  lag a release: the `claude_session_uuid` hook alias and its agent default,
  the `claude_resume_uuid` request alias and the agent inference that exists
  only for it, the `pty_sessions.current_claude_session_uuid` column, and the
  `repos` table that an older backend still writes. Remove each reader, and
  add the two dropping migrations, only once no live peer (node image, browser
  tab, or older backend against the same database) can still emit the old
  form. Source cleanup is not evidence about those peers.
- **Host recovery retirement.** Delete `nix/repair-existing-install.md` once a
  human confirms the dedicated host is off the retired `/etc/sulion` layout,
  and retire `sulion-stack-adopt` once the one-time container condition it
  documents is satisfied. The absence of `/etc/sulion` inside a PTY container
  proves nothing about the host.
- **Public-edge availability work.** Application resource limits, readiness
  and single-component recovery proof, and an archive insecure-transport
  policy were observed in the 2026-09-28 audit (F09–F11) and excluded from
  remediation at the user's direction. They are recorded there, not queued.
