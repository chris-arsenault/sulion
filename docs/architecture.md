# Architecture

Current shipped shape of sulion. Historical reasoning is at the bottom; the top half describes the system as it runs today.

Pointers into the code are the source of truth. This doc exists to orient a reader before they open `backend/src/` or `frontend/src/`.

## Shape

One Rust backend, one React frontend, one Postgres database, one bind-mounted dataset on the host.

```
PTY shell ──► xterm.js               (live: WebSocket bytes, no scrollback)
           └► JSONL file ──► ingester ──► Postgres ──► REST/WS ──► timeline pane
```

The live pane shows "now." All review happens in the structured timeline, sourced from the ingested transcript, not the terminal buffer.

## Session model

Two layers.

**PTY session** — a long-lived shell process on the server. Created per repo, survives client disconnect, dies only on explicit delete, shell exit, or server reboot. Has a backend-generated UUID.

**Agent session** — a single `claude` or `codex` invocation inside a PTY session, identified by the agent's own session UUID, backing exactly one JSONL file. A PTY session may hold zero, one, or many agent sessions sequentially.

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

- **Rail + sidebar** — repos and their PTY sessions, drag-resizable and pinnable. See `frontend/src/components/Sidebar.tsx`.
- **Work area** — tab-strip over two horizontal panes. Each tab is its own subtree keyed by `(session_id, view_kind)`. Terminal, timeline, file, diff, search, reference tabs all live here.
- **Mobile** — single-pane with drawer below 768px.

State management rules live in [`state-management.md`](state-management.md). Visual framework is in [`design.md`](design.md).

The **terminal pane** is the one React-opaque island: imperatively mounted `xterm.js`, direct WebSocket pipe, no React re-render on bytes.

The **timeline pane** uses `react-virtuoso` — overnight sessions produce thousands of events; a non-virtualized list is unusable.

## Backend surface

REST management (`GET/POST/DELETE` on sessions, repos, timeline, library, git, stats) plus WebSocket attach for live PTY streaming and event push. See `backend/src/api/routes.rs` for the authoritative route table.

WebSocket attach sends a snapshot rendered from the shadow `vt100` emulator on connect, then live-streams bytes. Inbound: keystrokes and `TIOCSWINSZ` resize. Multi-viewer is mirrored; inbound is last-writer-wins (single-user LAN tool).

## Deployment shape

Docker Compose, orchestrated by Komodo, on TrueNAS. Two images (`backend`, `frontend`), shared TrueNAS Postgres at `192.168.66.3:5432`, one bind-mounted dataset at `/mnt/apps/apps/sulion` → `/home/dev/` in the backend container. Full setup in [`deploy.md`](deploy.md).

## Historical reasoning

Kept because it still explains *why* the shape is what it is. Removed when it no longer matches the code.

- **Live vs history split.** Terminals aren't built to replay cursor-movement streams as scrollback. Separating rendering pipelines avoids ANSI-replay garbage and is the key ergonomic insight.
- **Web frontend, not native.** Zero-install on any LAN device beats the small latency edge a native terminal would give.
- **PTY runs a general shell.** Lets the user navigate, clone, run `claude --resume` manually. The PTY is the workbench.
- **Postgres, not SQLite.** The ahara ecosystem runs a shared TrueNAS Postgres; sulion joins it rather than running a sidecar.
- **Polling, not `LISTEN`/`NOTIFY`.** Claude responses span minutes; 1–2s timeline lag is imperceptible.
