# Session Broker — Design Document

## Intent

Build a server-side application that hosts persistent terminal sessions and provides a custom web frontend optimized for reviewing and interacting with Claude Code (and later Codex) sessions. Addresses two unmet needs:

1. **Persistence with reconnection from any LAN device** without the UI overhead of tmux/Zellij.
2. **Structured scrollback** that presents an agent session as a reviewable history of exchanges rather than a linear stream of terminal bytes.

The primary user runs long-lived agent sessions — some overnight, some conversational, some kept open for context continuity — and needs to be able to walk away, come back, and both continue interacting and review what happened while away.

## Core Architecture Decisions

### Single backend application
One server app handles PTY lifecycle, REST endpoints, WebSocket attach, and background ingestion of Claude Code transcripts. No microservices, no separate daemons. This is a personal tool on a LAN, not a distributed system.

### Web frontend, not native
Chosen because it deploys to every Windows machine on the LAN with zero install — just a bookmark. The native-terminal latency edge is real but small, and is outweighed by "works on every device immediately." Browser clipboard conventions (selection = copy, empty = SIGINT) are acceptable.

### PTY runs a general shell, not `claude` directly
Enables the user to navigate the filesystem, clone repos, run `claude --resume` manually, and generally treat the PTY as their remote workbench. Implies a two-layer session model (below).

### JSONL is source of truth; Postgres is the query layer; ingester owns the boundary
Claude Code's session transcripts at `~/.claude/projects/<project-hash>/<session-id>.jsonl` are append-only and written live by Claude Code itself. They survive daemon crashes and are authoritative by construction.

**Only the ingester reads JSONL.** All other consumers (REST API, WebSocket events pushed to the frontend) query Postgres. This concentrates all "messy reality" concerns — partial lines, unknown event types, parent/compaction session linking, format drift — into a single component with a single test surface. API handlers become trivial `SELECT` queries with no file I/O in the request path.

Trade-off: timeline freshness is bounded by ingester lag, not by the PTY stream. Acceptable because Claude responses take minutes; one-to-two-second timeline lag is invisible. The live PTY provides immediacy; the timeline provides reviewability.

### No terminal-side scrollback in the frontend
The live terminal pane only shows "now." All history lives in the structured timeline pane. This cleanly avoids the ANSI-replay-into-scrollback problem — terminals fundamentally aren't meant to replay cursor-movement streams into a linear buffer, and doing so produces visual garbage for any TUI application. Splitting "live" and "history" into separate rendering pipelines is the key ergonomic insight.

### The terminal pane must live outside React's reconciliation
xterm.js is mounted imperatively; the WebSocket pipes bytes directly to it; React never re-renders the terminal container in response to PTY data. React manages the timeline, sidebar, and chrome. This keeps keystroke latency indistinguishable from native SSH.

## Session Model

Two layers:

**PTY session** — a long-lived shell process on the server. Created when the user starts a new session in a given repo. Survives client disconnect. Dies only on explicit delete, shell exit, process crash, or server reboot. Identified by a backend-generated ID.

**Claude session** — a single `claude` invocation that ran inside a PTY session, identified by Claude Code's own session UUID and corresponding to exactly one JSONL file. A PTY session may contain zero, one, or many Claude sessions over its lifetime (sequentially; Claude Code is interactive and blocks the shell).

The UI's primary object is the PTY session. The dashboard/timeline filters to the *current* Claude session within that PTY by default but allows browsing prior ones.

### Session correlation
When the backend spawns a PTY shell, it injects `SULION_PTY_ID=<pty_id>` into the shell's environment. A Claude Code `SessionStart` hook reads that env var and posts `{pty_id, claude_session_uuid}` to a local Unix socket the backend listens on (e.g., `/run/sulion/correlate.sock` inside the container). The backend records the association in Postgres.

The correlation runs once per `claude` invocation. When the user starts a new Claude session in the same PTY, the hook fires again and updates the current-claude-session pointer.

### Compaction and parent sessions
Claude Code compaction spawns a new session UUID that references the prior one. Ingester schema includes a nullable `parent_session_uuid` column from day one so this linkage is preserved, even though compaction UI is deferred. Cheaper than a later migration.

### Dead sessions
If a PTY process dies, the session is unrecoverable. Mark as dead in the UI, distinguish clean-exit from crash where possible via exit status, and allow deletion. Do not attempt resurrection. Users can start a new PTY and run `claude --resume <uuid>` manually if they want to continue a previous Claude session.

## Backend Responsibilities

### REST endpoints (management)
- Create PTY session (in a given repo / working directory)
- List PTY sessions (with metadata: repo, state, age, current Claude session UUID if any)
- Delete PTY session
- Query structured history for a session (`GET /sessions/:id/history`) — returns timeline events from Postgres, supports pagination and filtering
- List repos (directory scan under a configured root)
- Create repo (clone or init)

### WebSocket endpoint (live attach)
Bidirectional PTY byte stream. On connect: backend sends current screen state (rendered via a headless terminal emulator — `vt100` crate or equivalent) so the user sees the actual TUI state rather than blank or garbage. After the snapshot, live-streams PTY output. Inbound messages are keystrokes and resize events (cols/rows → `TIOCSWINSZ` → SIGWINCH). Multiple concurrent viewers on one session are mirrored; inbound keystrokes from multiple viewers are last-writer-wins with no locking (single-user LAN tool).

**Shadow emulator lifecycle:** the headless emulator must be fed every PTY byte continuously for the full lifetime of the PTY session, including while no clients are attached. Otherwise the snapshot on reconnect lags. This is a persistent per-session memory/CPU cost; acceptable for a personal tool.

### Background ingester
Watches `~/.claude/projects/` (inside the container — see Deployment) for JSONL file changes via `notify` or polling. Parses new lines, writes structured events to Postgres indexed by Claude session UUID and timestamp.

**Partial-line handling:** only commit a line once it ends in `\n`. Buffer partial trailing bytes until the next write flushes them.

**Tolerance:** unknown event types are logged and skipped, not treated as errors. The JSONL format is an implementation detail of Claude Code, not a stable public API.

**Idempotency:** events are keyed on `(session_uuid, byte_offset)`. On ingester restart, read the max committed offset per session from Postgres and resume from there. Byte offset is stable because the files are append-only.

**Catch-up on startup:** scan all known JSONL files and replay any bytes past the stored offset.

### Schema guidance
Store events in a shape that makes timeline rendering and filtering cheap. Expect to query "all events in session X ordered by timestamp," "only tool_use events," "only user messages." Tool-call arguments and results stored as JSONB. Don't over-normalize — the structure of Claude Code's events is the natural schema. Include `parent_session_uuid NULL` from day one to preserve compaction linkage.

## Frontend Design

### Layout
Three regions, left to right:
1. **Sidebar** — repo tree. Under each repo, PTY sessions in that repo. Buttons for new session, new repo.
2. **Terminal pane** — live xterm.js connected to the selected session's PTY. No scrollback. Fills available vertical space.
3. **Timeline pane** — structured history. Collapsible blocks, one per user→Claude exchange.

Divider between terminal and timeline is draggable. Either pane can be collapsed to give the other full width.

### Timeline blocks
Each block represents one exchange. Collapsed view: single line with the user prompt summary, tool-use badges, and duration. Expanded view: full user message, full assistant response, tool calls individually expandable. Tool calls render in type-appropriate ways — file edits as diffs, bash as command + collapsible output, reads as path + preview, etc.

### Timeline freshness
Frontend polls `GET /sessions/:id/history` every 1–2 seconds (or on user action) for new events. Polling chosen over Postgres `LISTEN`/`NOTIFY` push for MVP simplicity; Claude responses span minutes, so a 1–2 second lag is imperceptible. Revisit if the UX ever demands sub-second timeline updates.

### Stats strip
Above the timeline: small chips showing tool-call counts, edit count, bash count, time since most recent compaction, session age. Clicking a chip filters the timeline.

### Filters
Chip toggles above the timeline: user-only, Claude-only, tools-only, errors-only. "User-only" is the explicit workflow for "show me just my prompts so I can click to jump to the corresponding Claude output."

### Virtualization
The timeline must be virtualized from day one. Overnight sessions will have thousands of events; a non-virtualized list will be unusable. Timeline blocks have variable heights (tool-call expansion, long outputs), so the virtualizer must support dynamic measurement — `react-virtuoso` or TanStack Virtual with measurement are both fine. Fixed-row virtualizers are out.

### Portrait orientation
Don't try to squeeze both panes into a narrow column. Detect portrait and switch to single-pane mode with a toggle between terminal and timeline at the top. Landscape remains two-pane.

### Keyboard behavior
Focus-based. Terminal focused = all keys go to PTY (xterm.js handles copy-vs-SIGINT based on selection state). Timeline focused = arrow keys navigate blocks, Enter expands, standard web conventions. Global command palette (Ctrl+K) for session switching and common actions.

## Deployment & Packaging

### Target: TrueNAS via Komodo (Docker Compose)
Sulion deploys as a Docker Compose stack on the TrueNAS server, orchestrated by Komodo — consistent with the rest of the ahara ecosystem. No systemd, no bare binaries. GHCR image, `compose.yaml`, Komodo handles pull/up/rollback.

### Multi-image packaging (backend + frontend)
Two images per the ahara shared-workflow convention for `rust + typescript` stacks:

- `backend/` — Rust server (PTY management, WebSocket, REST, ingester). Thick container: carries `git`, `bash`, `openssh-client`, `node`, and anything else the PTY shell needs as a baseline. Bind-mounts the dataset. `HOME=/home/dev`.
- `frontend/` — nginx serving the built React bundle. Proxies `/api` and `/ws` to backend. Browser sees a single origin — no CORS, same UX as single-image.

Both built by the shared workflow: Rust binary via `rust_artifacts.binaries` into `backend/dist/`, frontend via `pnpm run build` into `frontend/dist/`. Each Dockerfile COPYs pre-built artifacts — no in-container compilation.

### Dataset-backed workbench
TrueNAS hosts one dataset at `/mnt/apps/apps/sulion`. It is bind-mounted directly as `/home/dev` in the backend container — the dataset root *is* the dev user's home. No subpath layout to pre-create.

```
/mnt/apps/apps/sulion   →   /home/dev/
  .claude/                         (Claude creds, session JSONLs under projects/)
  .ssh/                            (keys for git clone — chmod 0700)
  .gitconfig
  .config/gh/                      (optional, if gh is added)
  .local/                          (user-installed tools — uv, pipx, npm -g prefix, cargo install)
  .cargo/
  repos/                           (workspace root — the new-repo API drops dirs here)
```

The container entrypoint (`backend/entrypoint.sh`) idempotently creates the expected subtree on first boot and pre-writes `settings.json` wiring the SessionStart hook. TrueNAS operator's whole bootstrap is one `zfs create` plus `chown 7321:7321` — no `mkdir -p`, no bootstrap script.

UID/GID is **7321**: deliberately unusual to dodge the 1000-series collision that most consumer container images cause. Pinned in `backend/Dockerfile` as the `DEV_UID`/`DEV_GID` build args; the dataset must be chowned to match.

Postgres data does not live here — the platform's shared TrueNAS Postgres owns its own storage at `192.168.66.3:5432`.

### Postgres: shared TrueNAS Postgres via ahara-db-migrate-truenas
The backend connects to `192.168.66.3:5432` (the platform's shared TrueNAS Postgres) using per-project credentials auto-provisioned by the `ahara-db-migrate-truenas` Lambda. Registration is a single-line addition to `var.truenas_db_projects` in `ahara-infra/infrastructure/terraform/services/db-migrate-truenas.tf`. On first deploy the Lambda creates the `sulion` database, an app role, and publishes credentials to SSM at `/ahara/truenas-db/sulion/{username,password}`. `secret-paths.yml` binds those SSM paths into the compose env — no manual SSM puts, no sidecar. This is the same pattern `nas-sonarqube` uses.

### Tool installation strategy
Base image carries `git`, `bash`, `curl`, `openssh-client`, `node` (for `npm i -g claude`), core toolchain. User-installed tools go to `~/.local/` which lives in the dataset — `uv tool install`, `pipx install`, `npm i -g` with prefix `/home/dev/.local`, `cargo install --root ~/.local/` all persist across container restarts and image rebuilds without Dockerfile edits. `claude` itself is installed this way so it can be pinned independently of the base image.

### JSONL path alignment
The ingester and the containerized PTY shell must see the same `~/.claude/projects/` path. Both run in the same container, both with `HOME=/home/dev`, so the Claude-recorded path and the ingester-watched path agree trivially. Get this wrong and the ingester sees nothing.

## Ahara Ecosystem Integration

### Standards followed
- **Stack:** Rust backend, TypeScript/React frontend, PostgreSQL 16, Docker, GitHub Actions shared workflow.
- **Repo layout:** `backend/`, `frontend/`, `compose.yaml`, `secret-paths.yml`, `Dockerfile`, `platform.yml`, `Makefile` (with `ci` target), `CLAUDE.md`, `README.md`, `LICENSE`, `scripts/deploy.sh`, `.github/workflows/ci.yml` (minimal caller invoking `chris-arsenault/ahara/.github/workflows/ci.yml@main`).
- **Project registration:** `platform.yml` declares `project: sulion`, `prefix: sulion`, `stack: [rust, typescript]`, `truenas: true`, `images: [backend, frontend]`, `rust_artifacts.binaries: [{bin: sulion, image: backend}]`. No `migrations` in the stack list because the sidecar Postgres is stack-local, not the shared platform RDS.

### Cross-repo registrations
- **`ahara-infra/infrastructure/terraform/control/project-sulion.tf`**: deployer role with `policy_modules = ["terraform-state", "komodo-deploy"]`. `module_bundles = []` (no ALB/website/cognito-app yet).
- **`ahara-infra/infrastructure/terraform/services/db-migrate-truenas.tf`**: add `sulion = { db_name = "sulion" }` to `var.truenas_db_projects`. This triggers the Lambda to provision DB + role + SSM creds on first deploy.

### Not needed for MVP
- **No `infrastructure/terraform/` directory** in this repo — nothing project-local to apply.
- **No reverse-proxy route.** LAN-only, bound to `192.168.66.3:30080`.
- **No manual Komodo stack creation.** The `deploy-truenas` GitHub Action calls `CreateStack` on every deploy, tolerant of already-exists.
- **No manual SSM puts.** DB credentials come from the Lambda; other params don't exist yet.

### Planned evolution path
When public exposure is needed, add in this order: (1) minimal `infrastructure/terraform/` in this repo for a Cognito client + reverse-proxy route; (2) a `reverse_proxy_routes` entry in `ahara-infra`'s network layer with `auth = "jwt-validation"` against the shared Cognito pool; (3) the `cognito-app` module for the frontend client. None of this blocks MVP; architecture accommodates it by not assuming the ALB path in any code.

### LAN-binding for MVP
Compose publishes the port on the TrueNAS LAN interface explicitly, not `0.0.0.0`, so a future misconfigured reverse proxy cannot accidentally expose an unauthenticated PTY spawner to the internet.

## MVP Scope

Implement:

- Backend: PTY spawn (with `SULION_PTY_ID` env injection), WebSocket attach with headless-emulator snapshot on connect, shadow emulator fed continuously, JSONL watcher with partial-line handling and `(session_uuid, byte_offset)` idempotency key, Postgres schema with `parent_session_uuid` column, Unix-socket correlation endpoint for `SessionStart` hook, REST endpoints (new/list/delete/history, repos list/create).
- Frontend: sidebar with repo tree, two-pane landscape layout, terminal pane working with xterm.js outside React, timeline pane rendering collapsible exchange blocks with tool-call expansion (virtualized), 1–2s polling refresh.
- Packaging: Dockerfile, compose.yaml with sidecar Postgres and dataset bind mounts, `platform.yml`, Makefile, minimal caller CI workflow, sample `SessionStart` hook.

Defer:

- Filter chips, stats strip, command palette
- Portrait layout
- Compaction detection UI (schema support is in from day one)
- Codex support (architecture should accommodate it; implementation later)
- Cross-session search and aggregation views
- Authentication (LAN-only is fine for now; Cognito/ALB path documented above)
- `infrastructure/terraform/`, shared RDS, reverse-proxy route

The MVP's job is to prove the architecture end-to-end and let the user run real overnight sessions against it. Polish and depth come after the shape is validated in real use.

## Known Gotchas

- **SIGWINCH handling** must be explicit. The web terminal's dimensions must flow to the PTY via `TIOCSWINSZ` on connect and on every resize, or Claude's TUI renders wrong.
- **Session correlation timing.** The `SessionStart` hook fires after Claude starts; brief window where a PTY exists but its current Claude session is unknown. UI should handle this gracefully (show the PTY as "claude starting" rather than erroring).
- **Transcript JSONL format is not a stable public API.** It's an implementation detail of Claude Code. Build the ingester to be tolerant of unknown event types (log and skip, don't crash) so upstream changes don't take the whole system down.
- **Alt-screen mode.** Claude's TUI uses the alternate screen buffer. The headless emulator used for snapshot-on-attach must handle alt-screen correctly or the snapshot will be wrong.
- **Repo directory scanning.** Needs to be cheap and reactive. Consider inotify or periodic refresh; don't re-scan on every sidebar render.
- **UID/GID alignment.** The container's `dev` user UID must match the owner of `/tank/dev/sulion/` on TrueNAS, or writes from the PTY will appear with wrong ownership on the host dataset.

## Reasoning Reference

Architectural choices were driven by these constraints, in priority order:

1. **Interactive conversation with Claude Code must work perfectly.** This rules out anything that doesn't give Claude a real PTY. Structured-event-streaming modes (`--output-format stream-json`) are batch-only and can't replace the interactive REPL.
2. **Scrollback must be better than a terminal provides, not worse.** Forces separation of live and historical rendering. Drives the two-pane design and the "terminal has no scrollback" decision.
3. **Zero per-device install friction on Windows LAN clients.** Forces web frontend.
4. **Latency must feel native.** Forces terminal-outside-React and direct WebSocket-to-xterm.js wiring.
5. **The tool should feel purpose-built for Claude Code sessions, not a generic terminal.** Drives the custom timeline UI with tool-aware rendering rather than just a log viewer.
6. **Ecosystem consistency with ahara.** Drives Docker/Komodo deploy, Rust/TS stack, shared CI workflow. The dataset-backed workbench pattern makes containerization a clean fit for this host-intimate service rather than a fight against it.
