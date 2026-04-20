# Changelog

User-visible feature history. System redesigns, refactors, lint rollouts, and visual-polish passes are deliberately excluded — see git log for that.

## Unreleased

### Timeline and review

- **Repo timeline.** Right-click a repo → one merged chronological feed of every correlated agent session in the repo, including sessions whose PTY is already deleted. Session badges render inline with the other timeline metadata.
- **Future prompts.** Queue a deferred follow-up against the active agent invocation without breaking its run. Survives refresh and backend restart; explicit "send" injects into the terminal.
- **File traceability.** Click a file reference anywhere in the timeline to open its file or diff tab; open a file and see every turn that touched it.
- **Turn grouping.** The timeline groups events into turns with collapsible detail, filter chips, subagent drill-in, thinking fly-out, tool hover card, sticky turn header, and an inspector side pane.
- **Copy as markdown.** Per-event and per-turn copy actions on every timeline row.
- **Filter chips.** Tool type, file path, errors-only, speaker, sidechain, bookkeeping — applied at the turn level.

### Sessions

- **Rename, pin, and color.** Per-session, persisted, in the sidebar.
- **Unread indicator.** Bell / dot per session row when new agent activity lands while you're elsewhere.
- **Dead and orphaned session handling.** Dead PTYs are marked; orphaned agent sessions offer a `Resume from orphaned` action that spawns a fresh PTY and `claude --resume`s into them.
- **Codex support.** Codex invocations are ingested, correlated, and reviewable through the same timeline surface as Claude.

### Workspace

- **Tabs.** File, diff, search, and reference tabs alongside terminal and timeline. Move tabs between panes; state persists across reloads.
- **Repo-centric sidebar.** Repos group sessions, with a git staleness signal per repo.
- **Context menus.** Right-click on tree nodes, session rows, tab handles, timeline turns, tool renderers, and library entries — one consolidated menu layer.
- **Command palette.** Cmd/Ctrl-K for session switching and navigation.
- **Mobile single-pane mode.** Drawer sidebar and pane tabs below 768px.

### Files and git

- **FileTab viewer.** Syntax highlighting, JSON / NDJSON explorer, raw toggle.
- **Paste-as-file.** Large pastes in the terminal auto-upload to the session's working dir instead of choking the PTY.
- **Diff tab.** Per-file diff view with stage buttons; opened from dirty files in the sidebar or timeline references.

### Library

- **Prompts.** Save, browse, edit, and inject reusable prompts into the active terminal.
- **References.** Pin or hoist assistant outputs as standalone reference artifacts, openable as tabs.

### Observability

- **Stats strip.** Tool-call counts, edit count, bash count, session age, token usage above the timeline.
- **Sidebar resource panel.** Backend memory / CPU, Postgres size, event counts.

### Terminal

- **5000-line scrollback** in `xterm.js`.
- **Clipboard fixes.** `Ctrl+V` pastes, `Ctrl+Shift+V` no longer double-pastes.
