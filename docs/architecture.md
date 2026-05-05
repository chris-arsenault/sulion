# Architecture

Current shipped shape of sulion. Historical reasoning is at the bottom; the top half describes the system as it runs today.

Pointers into the code are the source of truth. This doc exists to orient a reader before they open `backend/src/` or `frontend/src/`.

## Shape

One Rust backend, one React frontend, one Rust secret broker, one Rust Docker
runner, two Postgres databases, and several explicitly mounted TrueNAS datasets.

```
PTY shell ──► xterm.js               (live: WebSocket bytes, no scrollback)
           └► JSONL file ──► ingester ──► Postgres ──► REST/WS ──► timeline pane

browser UI ──► frontend ──► backend ──► PTY runtime
          └──► /broker/* ──► secret broker ──► sulion_broker Postgres
```

The live pane shows "now." All review happens in the structured timeline, sourced from the ingested transcript, not the terminal buffer.

The broker exists to keep secret storage and unlock state out of the PTY runtime and out of the main backend. General app data lives in the main `sulion` database; encrypted secret bundles and grant state live in the separate `sulion_broker` database.

## Session model

Two layers.

**PTY session** — a long-lived shell process on the server. Created per repo, survives client disconnect, dies only on explicit delete, shell exit, or server reboot. Has a backend-generated UUID.

**Agent session** — a single `claude` or `codex` invocation inside a PTY session, identified by the agent's own session UUID, backing exactly one JSONL file. A PTY session may hold zero, one, or many agent sessions sequentially.

**Workspace** — the filesystem checkout bound to a PTY session. A workspace is
either the canonical repo checkout (`main`) or a Sulion-created Git worktree on
an isolated branch. PTYs run with their cwd inside the workspace and receive
`SULION_WORKSPACE_*` metadata so agents can tell where they are.

The UI's primary object is the PTY session. The timeline defaults to the current agent session within that PTY, and the repo timeline (#56) merges every correlated agent session in a repo into one chronological feed.

### Correlation

The backend injects `SULION_PTY_ID=<pty_id>` into the PTY's environment. A `SessionStart` hook posts `{pty_id, agent_session_uuid}` to `/run/sulion/correlate.sock`; the backend records the association in Postgres and updates the current-agent-session pointer on every new invocation.

Hook source: `backend/hooks/session-start.sh` (installed into the container at `/opt/sulion/hooks/session-start.sh`). Correlation is best-effort — hook failure is silent; the JSONL still ingests.

### Compaction

Schema carries a nullable `parent_session_uuid` from day one so a compacted child agent session links to its parent. Cheap now; avoids a migration when compaction UI lands.

### Dead and orphaned sessions

Dead PTYs are marked in the UI and cannot be resurrected. "Orphaned" agent sessions (transcript present, no owning PTY) get a `Resume from orphaned` action that spawns a fresh PTY and runs `claude --resume <uuid>`.

## Ingestion

JSONL is source of truth. Postgres is the query layer. The ingester owns the boundary — see [`ingestion.md`](ingestion.md) for why and for the future runtime-split plan.

Code boundary:

- `backend/src/ingest/ingester.rs` — transcript polling + orchestration
- `backend/src/ingest/canonical/` — source-specific (Claude, Codex) translation into the canonical event schema
- `backend/src/ingest/timeline/` — app-shaped timeline projection
- `backend/src/ingest/projection.rs` — materialization into `timeline_*` tables

## Invariants — do not break

1. **Only the ingester reads JSONL.** REST handlers and WebSocket event pushes query Postgres. Never `fs::read` the `~/.claude/projects/` files from the request path.
2. **The terminal pane lives outside React's reconciliation.** `xterm.js` is mounted imperatively; WebSocket bytes pipe directly. React must not re-render the terminal container in response to PTY data.
3. **The ingester tolerates partial lines and unknown event types.** Only commit on trailing `\n`. Log and skip unknown types. JSONL format is not a stable public API.
4. **The shadow terminal emulator is fed continuously**, including while no clients are attached. Otherwise snapshot-on-reconnect lags.
5. **Ingester idempotency key: `(session_uuid, byte_offset)`.** JSONL is append-only, so byte offset is stable.
6. **Schema carries `parent_session_uuid NULL`** from day one.

## Frontend shape

Three surfaces, one work area.

- **Rail + sidebar** — repos, PTY sessions, and isolated workspaces with resume/diff/delete actions. See `frontend/src/components/Sidebar.tsx`.
- **Work area** — tab-strip over two horizontal panes. Each tab is its own subtree keyed by `(session_id, view_kind)`. Terminal, timeline, monitor, file, diff, reference, and secrets-management tabs all live here.
- **Mobile** — single-pane with drawer below 768px.

State management rules live in [`state-management.md`](state-management.md). Visual framework is in [`design.md`](design.md).

The **terminal pane** is the one React-opaque island: imperatively mounted `xterm.js`, direct WebSocket pipe, no React re-render on bytes.

The **timeline pane** uses `react-virtuoso` — overnight sessions produce thousands of events; a non-virtualized list is unusable.

The **secrets tab** is the env-bundle setup surface. PTY-scoped grants with TTL live in terminal/session context menus, and the actual unlock and storage boundary lives in the broker. See [`secrets.md`](secrets.md).

## Backend surface

REST management (`GET/POST/DELETE` on sessions, repos, timeline, library, git, stats) plus WebSocket attach for live PTY streaming and event push. See `backend/src/api/routes.rs` for the authoritative route table.

Repo-scoped filesystem/git routes target the canonical checkout. Workspace-scoped
routes under `/api/workspaces/:id/*` target the session-bound checkout/worktree.
Deleting an isolated workspace removes the Git worktree registration, optionally
deletes its Sulion branch, and marks the workspace row deleted; main workspaces
are not deletable.

WebSocket attach sends a snapshot rendered from the shadow `vt100` emulator on connect, then live-streams bytes. Inbound: keystrokes and `TIOCSWINSZ` resize. Multi-viewer is mirrored; inbound is last-writer-wins (single-user LAN tool).

The backend also launches PTYs with Sulion-managed wrapper tools on `PATH`:

- `cl` / `co` for correlated Claude/Codex startup
- `with-cred` for general env-bundle injection
- `aws` as a wrapper over the real AWS CLI
- `docker` as a constrained runner client

`with-cred` and `aws` are the only supported secret-consumption paths. The backend does not own the broker master key and does not expose any alternate secret-injection mechanism.

## Broker surface

The broker is a separate Rust service and container. It stores encrypted secret payloads, tracks active grants, validates direct browser requests for secrets/grants management, and redeems active grants for wrapper use.

Its responsibilities are intentionally narrow:

- store env-bundle secrets
- manage PTY-scoped grants with TTL
- redeem grants for `with-cred` and `aws`

It does not run PTYs, ingest transcripts, or serve the main application API.

## Container Runner

The runner is a separate Rust service and container. It is the only Sulion
container with the host Docker socket mounted. PTYs see a `docker` wrapper that
sends the current working directory, PTY id, and argv to the runner. The runner
executes the Docker CLI from the same mounted workspace path after applying
Sulion policy: supported subcommands only, Sulion labels on created containers,
resource defaults, no privileged mode, no host namespaces, no extra caps, no
devices, no bind mounts, automatic attachment to the `sulion` Docker network,
and no interactive `-it` sessions. Compose commands go through a shim that maps
Compose's default network to the same external `sulion` network.

The runner is intentionally a command broker, not a Docker API proxy. A runner
compromise is equivalent to host Docker socket compromise; an agent compromise
is bounded by runner policy.

## Deployment shape

Docker Compose, orchestrated by Komodo, on TrueNAS. Four images (`backend`, `broker`, `runner`, `frontend`), shared TrueNAS Postgres at `192.168.66.3:5432`, backend state under `/mnt/apps/apps/sulion`, canonical repos under `/mnt/apps/apps/sulion/repos`, isolated worktrees under `/mnt/apps/apps/sulion/workspaces`, and broker key material under `/mnt/apps/apps/sulion-broker`. Full setup in [`deploy.md`](deploy.md).

## Historical reasoning

Kept because it still explains *why* the shape is what it is. Removed when it no longer matches the code.

- **Live vs history split.** Terminals aren't built to replay cursor-movement streams as scrollback. Separating rendering pipelines avoids ANSI-replay garbage and is the key ergonomic insight.
- **Web frontend, not native.** Zero-install on any LAN device beats the small latency edge a native terminal would give.
- **PTY runs a general shell.** Lets the user navigate, clone, run `claude --resume` manually. The PTY is the workbench.
- **Postgres, not SQLite.** The ahara ecosystem runs a shared TrueNAS Postgres; sulion joins it rather than running a sidecar.
- **Polling, not `LISTEN`/`NOTIFY`.** Claude responses span minutes; 1–2s timeline lag is imperceptible.
