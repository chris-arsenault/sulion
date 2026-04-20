# E2E Coverage Plan

Planning-oriented view of the real-stack Playwright suite. This is meant to answer:

1. What high-level product areas are already covered end to end?
2. Which areas are only partially covered?
3. What is the next best test to add?

Status values:

- `Covered`: the main user path is exercised in a stable end-to-end way
- `Partial`: some meaningful path is covered, but important branches or variants are still missing
- `Missing`: no meaningful e2e coverage yet
- `Out of scope`: intentionally excluded from this suite

Current specs:

- `01` = `frontend/e2e/01-navigation-and-tabs.spec.ts`
- `02` = `frontend/e2e/02-timeline-and-workspace.spec.ts`
- `03` = `frontend/e2e/03-codex-and-stats.spec.ts`
- `04` = `frontend/e2e/04-library-and-terminal.spec.ts`
- `05` = `frontend/e2e/05-mobile.spec.ts`
- `06` = `frontend/e2e/06-mock-terminal.spec.ts`
- `07` = `frontend/e2e/07-agent-roundtrip.spec.ts`

| Feature Area | Status | Priority | Current Coverage | Next Test To Add |
|---|---|---|---|---|
| Stack boot: backend + frontend + Postgres + seeded ingest | Covered | High | All specs run on the real stack, with Postgres and the backend now isolated in Docker. Claude/Codex JSONL is written, correlated, ingested, and then exercised through the UI. | Add one explicit seed-health smoke assertion so stack failures fail earlier and more readably. |
| Session discovery and navigation | Covered | High | `01`, `03`, `04`, `05` cover command-palette open, sidebar selection, and mobile drawer selection. | Add direct repo-jump and session unread-state assertions once that behavior is stable enough to test. |
| Session metadata management | Covered | Medium | `01` covers rename, pin/unpin, and color assign/clear from the sidebar. | Add reload persistence for renamed/pinned/colored session state. |
| Workspace tabs and pane movement | Partial | High | `01` covers file/diff tab open, moving a file tab across panes, and reload persistence of restored workspace tabs. | Add tab close/reopen, context-menu move-to-pane, and active-tab restoration assertions. |
| Timeline inspection: Claude path | Covered | High | `02`, `04`, and `07` cover turn selection, prompt detail, thinking flyout, save actions, and a live mock roundtrip that emits Claude-shaped assistant/edit/websearch/subagent JSONL into ingest. | Add multi-turn behavior once seed data includes more than one meaningful Claude turn. |
| Timeline inspection: Codex lineage path | Covered | High | `03` and `07` cover the Codex task row, subagent modal, lineage drill-down, and a live mock roundtrip that emits Codex-shaped assistant/edit/websearch/subagent JSONL into ingest. | Add a richer Codex sidechain scenario with multiple delegated tasks and parent/child switching. |
| Timeline filtering and faceting | Partial | High | `02` covers file-path filtering and recovery via `Show all`. | Add speaker/category/error/sidechain filter combinations and persistence of filter state. |
| File browser and nested tree navigation | Covered | Medium | `01` and `02` cover nested path expansion and opening tree files into tabs. | Add directory collapse/re-expand and dirty-file-only navigation cases. |
| File viewers | Partial | Medium | `02` covers JSON rendering and raw-mode toggle; `01` indirectly covers diff-open adjacency from a dirty file. | Add markdown, code-highlighted, ndjson, truncated, binary, and image viewer cases. |
| File trace integration back into timeline | Covered | Medium | `02` covers related-turn display and jump-back into the timeline from the file tab. | Add multiple related touches so ordering and disambiguation are tested. |
| Git review workflow | Partial | High | `01` covers opening a diff tab from a dirty file. | Add stage/unstage from `DiffTab`, full-repo diff, and clean-working-tree transitions. |
| Library: prompts and references | Partial | High | `04` covers save prompt, inject into a live mock terminal, save reference, and open reference tab. | Add prompt editing, prompt deletion, reference deletion, and behavior with no active terminal tab. |
| Terminal integration surfaces | Covered | Medium | `04`, `06`, and `07` cover prompt injection, snapshot-on-attach, typed input echo, streamed output, resize handling, paste-as-file, reconnect, dead-session UI, and mock agent launch from a real shell session. | Add right-click paste and selection-copy browser coverage if those interactions prove stable enough under Playwright. |
| Stats / observability strip | Partial | Medium | `03` covers stats-strip visibility and expanded detail fields. | Add refresh/update behavior and thresholds or warning-state assertions once those semantics are stable. |
| Mobile shell | Covered | Medium | `05` covers drawer open/close and switching to a timeline tab on mobile Chromium. | Add mobile file/ref navigation and drawer interactions with multiple open sessions/tabs. |
| Repo creation flow | Missing | High | Not covered by Playwright today. | Add a create-repo scenario against a temporary seeded git target or a local bare repo fixture. |
| Session creation flow | Missing | High | Not covered by Playwright today. | Add create-session from sidebar and verify new terminal/timeline tabs appear. |
| Failure and recovery paths | Partial | High | `06` now covers websocket reconnect and terminal exit/ended-session recovery. | Add API failure rendering and empty-state regressions. |
| Search surfaces beyond command palette | Missing | Medium | Only command-palette session open is covered today. | Add timeline search when implemented; until then, keep this explicitly unowned. |
| Actual live agent integration | Out of scope | High | Intentionally excluded by requirement. The suite drives mock Claude/Codex launchers that emit deterministic JSONL into isolated transcript roots, not the real token-using agents. | None. Keep excluded unless requirements change. |

## What This Means Right Now

- The suite is strong on the core read/review workflow: ingest seeded transcripts, inspect them in the UI, move around the workspace, and save useful artifacts.
- The new launcher-backed roundtrip path gives direct browser coverage for Claude/Codex ingest parity without calling the real agents.
- The next highest-value additions are the write-heavy operational paths: repo/session creation, git staging, and failure handling.
- File viewing is only partially covered; JSON works, but viewer-format breadth is still a clear gap.

## Suggested Next Additions In Order

1. Add a `DiffTab` stage/unstage test. It covers a real workflow and would exercise a path that currently has no browser coverage.
2. Add a session-creation test. That validates the UI path most likely to break when backend/session wiring changes.
3. Add a repo-creation test. That closes the last obvious top-level sidebar action gap.
4. Expand timeline filter coverage beyond file-path filtering to speaker/category/error combinations.
5. Add file-viewer format coverage for markdown, ndjson, binary/truncated files, and image/svg behavior.

## Explicitly Excluded From This Plan

- Live Claude/Codex agent execution itself
- Cross-browser matrix work beyond Chromium
- Auth and multi-user concerns
- Long-running chaos or soak coverage beyond targeted reconnect/failure tests
