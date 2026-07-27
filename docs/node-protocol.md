# Control-plane and development-node contract

Status: Phase 4 transport and persistence foundation shipped; development
runtime extraction remains scheduled for Phase 5.

This document defines the runtime seam selected by
[ADR 0002](adrs/0002-hybrid-control-plane-and-dev-node.md). Phase 4 selected a
bounded JSON protocol over WebSocket for the long-lived channel and proved its
identity, heartbeat, compatibility, replay, and reconciliation behavior. Phase
5 adds the typed PTY, terminal-stream, repository, and workspace messages to
that channel. The ownership and failure behavior below remains the target for
that extraction.

## Terms

- **Control** — the durable, browser-facing Sulion API on TrueNAS.
- **Node** — the machine-local Sulion runtime on the dedicated development
  host.
- **Node connection** — one outbound-established, mutually authenticated,
  long-lived channel from a node to control.
- **Node boot** — one operating-system boot/runtime incarnation, identified
  separately from stable node identity.
- **Operation** — one typed, idempotent control request and its durable result.
- **Attachment** — one browser's temporary view of a node-owned PTY.

The initial product permits one active node. Protocol and persistence still use
an explicit `node_id`; “the server” is not an implicit global identity.

## Ownership rule

A responsibility belongs on the node if it needs a local process handle,
repository/workspace bytes, agent transcript bytes, or the development Docker
daemon. A responsibility belongs on control if it is durable coordination,
browser authentication, global query/projection state, or independent of local
source bytes.

No runtime may infer ownership from reachability. Ownership is declared by the
operation or durable row and enforced at both ends.

## Current API ownership inventory

The target owner column accounts for every route registered by
`backend/src/api/mod.rs`, `backend/src/api/routes.rs`, and
`backend/src/api/device_routes.rs`.

| Current surface | Target owner | Behavior after split |
|---|---|---|
| `GET /health` | Control | Reports control/DB health; node health is a separate field/surface. |
| `GET /api/app-state` | Control | Queries durable state and the node registry; never calls a local `PtyManager`. |
| `POST /api/ws-tickets` | Control | Keeps the existing Cognito-authenticated, short-lived, one-use ticket. |
| `GET /ws/sessions/:id` | Split | Control authorizes and bridges; the owning node supplies snapshot/live bytes and accepts input/resize. |
| `POST /api/sessions` | Split | Control validates and records an idempotent operation; node resolves local workspace and spawns. |
| `DELETE /api/sessions/:id` | Split | Control authorizes and records; node terminates and reports the process result. |
| `PATCH /api/sessions/:id` | Split | Durable metadata stays on control; process-affecting metadata is forwarded to the node. |
| `POST /api/sessions/:id/agent` | Node | Typed launch operation in the existing PTY environment. |
| `POST /api/sessions/:id/agent/interrupt` | Node | Typed signal operation; no arbitrary PID or signal is accepted from the browser. |
| `POST /api/sessions/:id/prompt` | Node | Typed byte/prompt delivery to the owning PTY. |
| `POST /api/sessions/:id/e2e/drop-ws` | Control | Test-only attachment close; does not affect the node PTY. |
| `GET /api/sessions/:id/history` | Control | Postgres query. |
| Session/repo timeline and turn routes | Control | Postgres query and projection only. |
| Session future-prompt routes | Control | Durable queue CRUD; delivery becomes a typed node prompt operation. |
| `POST /api/repos` | Split | Control validates identity; node creates/imports local data and returns discovered facts for commit. |
| Repo rename/delete routes | Split | Control enforces durable relationships; node performs the quiesced filesystem/Git mutation. |
| Repo refresh/dirty-path routes | Node | Local Git/status operation with a typed result. |
| Repo file/raw/file-trace routes | Split | Node supplies bounded file content; control may enrich trace results from Postgres. |
| Repo diff/stage/upload routes | Node | Local, allowed-root-scoped filesystem/Git operation. |
| Device repo ingest/raw routes | Node | Control authenticates the device token and forwards a bounded repo operation. |
| Repo timeline routes | Control | Postgres query. |
| Workspace list/get | Control | Durable rows, enriched with last reported node state. |
| Workspace delete/refresh/dirty/file/diff/stage/upload | Node | Typed, workspace-ID-scoped operations; control records returned state. |
| Plan and plan-event routes | Control | Postgres-backed durable coordination. |
| `POST /api/monitor/timeline` | Control | Postgres query. |
| `GET /api/metrics` | Control | Durable aggregation plus last reported node telemetry. |
| Library routes | Control | Durable control-plane library storage; node receives prompt/reference content through typed operations. |
| `POST /api/admin/reindex` | Control | Coordinates projection maintenance and node-ingester state without reading JSONL. |
| `POST /api/admin/retrieval/reindex` | Control | Existing retrieval-service operation. |
| Device pairing and approval routes | Control | Existing public/Cognito/device-token model. |

The broker's `/broker/*` surface remains a separate TrueNAS service and is not
part of the node protocol. The node only performs the existing signed
per-PTY registration/revocation flow.

## Background-task ownership inventory

| Current task | Target owner | Notes |
|---|---|---|
| Main SQLx migrations | Control | Exactly one migration owner. |
| Blanket PTY orphan reconciliation at API startup | Scoped legacy-only | During migration it touches only `node_id IS NULL`; node-owned rows use boot inventory and the control-only role has no local rows. |
| Correlation/activity Unix socket | Node | The socket and hooks remain local to PTYs. |
| Transcript polling | Node ingester | The only JSONL reader. |
| Canonical/timeline repair from existing Postgres payloads | Control maintenance | Does not require transcript files. |
| Ingest projection scheduling requiring new local lines | Node ingester | Commits byte offset with projected writes. |
| Usage backfill from stored events | Control maintenance | Postgres-only. |
| Repository discovery/status manager | Node | Local filesystem and Git. |
| Workspace reconciliation/status manager | Node | Local worktree and Git state. |
| Host CPU/memory/process sampling | Node | Reported as timestamped node telemetry. |
| Durable metrics aggregation | Control | Postgres plus last reported node telemetry. |
| Retrieval indexing/backfill | Control/retrieval service | Postgres and TrueNAS embedding service. |
| Code discovery/index/LSP workers | Node/code-intelligence service | Local read-only source roots. |
| Constrained Docker runner | Dev node, brokered policy only | Absent in direct mode. |
| PTY process reader/writer/supervisor/emulator tasks | Node | Never reconstructed by control. |

## Connection and authentication

- The node initiates the connection to an authenticated control endpoint.
- Enrollment creates a stable node ID and a node-specific asymmetric
  credential.
- The private credential is delivered as a root-owned systemd credential or
  equivalent host secret and is not stored in the Nix store or `/home/dev`.
- Control stores the public identity and explicit revocation state.
- Every new connection proves stable node identity and sends a fresh node boot
  ID.
- Transport encryption and application authentication are both required even
  on the LAN/WireGuard path.
- Browser JWTs and WebSocket tickets are terminated at control and never
  forwarded as node credentials.
- A node credential authorizes only the typed node protocol. It cannot manage
  broker secrets, deploy the control plane, or perform arbitrary SQL.

### Shipped enrollment lifecycle

Control exposes three credential-lifecycle operations:

- authenticated `POST /api/nodes/enrollment-tokens` creates a short-lived,
  single-use token; an optional `target_node_id` makes it a rotation token;
- public `POST /api/nodes/enroll` atomically consumes the token and registers
  a 32-byte Ed25519 public key; and
- authenticated `POST /api/nodes/:id/revoke` revokes the key and terminates
  the current node connection.

Only the token hash is stored. Public credential generations retain their
fingerprint, replacement time, and revocation time for audit. Rotation
increments the generation and invalidates the old connection before the
replacement key may connect. Revocation cannot be reversed by the node. The
node-side private key file and automated enrollment command arrive with the
`sulion-node` binary in Phase 5; until then these endpoints are exercised by
integration clients and the standalone runtime uses an explicitly marked
internal identity.

The long-lived endpoint is `GET /ws/nodes`. Control sends a fresh random
challenge. The node signs a canonical handshake containing that challenge,
stable node ID, boot ID, build/version range, path contract, Docker policy,
release digest, and sorted capabilities. Control verifies the signature
against the enrolled public key before recording any compatible connection.
Direct deployments must expose this endpoint only through TLS (`wss`);
production proxy wiring is part of the control-plane phase.

## Version handshake

The initial handshake carries:

- stable `node_id`;
- fresh `boot_id`;
- node build Git SHA;
- node protocol version;
- supported control protocol range;
- declared capabilities;
- Docker policy and daemon information;
- canonical path contract version; and
- observed release-manifest digest.

Control replies with:

- accepted/rejected status and a stable reason code;
- control build Git SHA and protocol version;
- accepted node capability set;
- desired release digest;
- heartbeat interval and timeout; and
- outstanding drain or reconciliation request.

An incompatible node may remain visible for diagnostics but receives no
mutating commands.

## Envelope

Every message has a typed envelope equivalent to:

```text
protocol_version
node_id
boot_id
message_id
message_kind
request_id?
operation_id?
session_id?
workspace_id?
stream_id?
sequence?
payload
```

The wire representation is tagged JSON over WebSocket. Each frame is limited
to 256 KiB. Protocol version 1 is the only currently accepted version and path
contract version 1 is mandatory. Unknown message kinds from an otherwise
compatible peer are ignored and logged; malformed required fields terminate
that authenticated connection.

Connection generations are independent from stable node and boot identity. A
new connection gets a random `connection_id`; database updates and disconnect
handling compare it before changing state. A delayed close from an older
socket therefore cannot overwrite a successful reconnect.

## Operations

Mutations are represented by a durable control-side operation:

```text
operation_id
idempotency_key
node_id
kind
resource_id
requested_at
dispatched_at?
completed_at?
status = pending | dispatched | succeeded | failed | canceled
result?
error_code?
```

The node keeps a bounded result cache keyed by `operation_id` for the current
boot and returns the prior result when control replays a completed operation.
Operations that create a durable resource use IDs allocated by control before
dispatch, so replay cannot create a second PTY or workspace.

No operation accepts an arbitrary executable, host path, PID, signal, Docker
request, or shell fragment from the public API. Agent launch variants and
filesystem scopes are explicit protocol types.

Phase 4 ships `probe_echo` and `reconcile_inventory` as non-host-mutating
operations that exercise the complete durable dispatcher. The operation table
preallocates the operation ID, enforces `(node_id, idempotency_key)`
uniqueness, records every dispatch boot and attempt, and retains terminal
result/error data. The standalone loopback node keeps the same bounded
current-boot result cache required of the extracted node. Phase 5 extends the
closed operation enum; it does not introduce a generic command operation.

## Terminal streams

The node owns:

- PTY master handles;
- output broadcast channels;
- input and resize channels;
- process supervision; and
- the continuously fed shadow emulator.

Attach flow:

1. Browser obtains a one-use ticket from control.
2. Browser connects to control `/ws/sessions/:id`.
3. Control resolves `node_id`, authorizes the session, and opens an attach
   stream.
4. Node renders and sends a snapshot before live bytes for that attachment.
5. Control bridges binary output and typed input/resize/ping messages.
6. Detach closes only the attachment.

Each node-to-control stream frame carries `stream_id` and a monotonic
`sequence`. Sequence detects a broken attachment; it is not persisted terminal
scrollback. Reattach always starts from a newly rendered shadow snapshot.

Backpressure must bound memory per attachment. A slow browser may lose its
attachment and reconnect; it must not stall the PTY reader or shadow emulator.

## Reconciliation

Control restart:

- node retains processes and reconnects;
- control does not mark sessions dead during startup;
- node reports stable node ID, new or existing boot ID, and live PTY IDs;
- control compares durable rows with the reported inventory.

Same boot ID:

- reported live IDs remain live;
- missing IDs require an explicit node result or timeout policy before being
  marked dead;
- unknown node IDs are quarantined until reconciled.

Changed boot ID:

- PTYs from the previous boot cannot be live in the first implementation;
- their rows become lost/dead with a node-reboot reason;
- repositories and transcript roots are rediscovered without replaying
  committed event offsets.

No node connection:

- control retains durable session/history state;
- new local mutations return `503` with a node-unavailable code;
- read-only Postgres surfaces continue working;
- the UI shows last heartbeat and observed release without estimating live
  process state.

The legacy startup orphan pass now updates only rows with `node_id IS NULL`.
Node-owned live rows survive a control restart. A node heartbeat may carry a
complete live-session inventory for its current boot. A new boot marks only
live rows owned by the previous boot dead with a `node_reboot` runtime end
reason; heartbeat expiry records node disconnect timestamps without changing
PTY state.

## Standalone loopback

The combined backend defaults to `SULION_NODE_TRANSPORT=loopback`. It enrolls
the stable internal node
`00000000-0000-0000-0000-000000000001`, establishes an in-memory outbound
connection, sends heartbeats, and executes operations through the same durable
dispatcher and result-cache rules as a remote node. Existing PTY, repository,
workspace, and ingester calls remain in-process during Phase 4.

Set `SULION_NODE_TRANSPORT=remote` on a control-only process to disable the
internal node. `SULION_DEPLOYMENT_ROLE=control-plane` selects that default.
For standalone deployments, `SULION_STANDALONE_NODE_ID` and
`SULION_STANDALONE_NODE_NAME` override the stable internal identity. These
settings are a migration seam, not a second application implementation.

## Ingestion

Only the node ingester reads:

- `/home/dev/.claude/projects`; and
- `/home/dev/.codex/sessions`.

It continues to tolerate partial lines and unknown event types. A byte offset
advances only after the corresponding canonical/timeline transaction commits.
Network or database failure therefore leaves the local append-only source
available for retry.

Projection repair that operates solely on stored `events.payload` is
control-owned maintenance. Parser/projection versions must be compatible with
the node release during rolling deployment; only one component owns each
version gate and backfill cursor.

## Filesystem safety

Typed file/Git/workspace operations receive repo or workspace IDs plus
normalized relative paths. The node:

- resolves the durable ID to a registered local root;
- canonicalizes and validates the requested path;
- rejects traversal and symlink escapes;
- enforces bounded upload/read/response sizes;
- never accepts a caller-supplied allowed root; and
- reports local revision/fingerprint with mutation results.

Repository rename/delete and worktree deletion keep the existing dirty/live
session guards. Control authorizes durable relationships; node validates
current filesystem state immediately before mutation.

## Behavioral regression baseline

Phase changes must preserve these existing behavioral suites:

| Contract | Existing evidence |
|---|---|
| PTY spawn, input, output, resize, metadata, and exit | `backend/tests/pty_integration.rs` |
| Ticketed attach, terminal snapshot, reconnect, and multi-view behavior | `backend/tests/ws_integration.rs`, `frontend/e2e/06-mock-terminal.spec.ts` |
| Claude/Codex correlation and activity | `backend/tests/correlate_integration.rs` |
| Partial-line, unknown-event, offset-idempotent, repair, and lineage ingest | `backend/tests/ingester_integration.rs` |
| Main and isolated workspace lifecycle, dirty state, Git, and deletion guards | `backend/tests/workspace_integration.rs` |
| REST history/timeline/repo/session behavior | `backend/tests/rest_integration.rs` |
| Broker request validation, grants, and credential redemption | broker unit/integration targets registered by `scripts/run-backend-integration-tests.sh` |
| Retrieval auth/search/backfill behavior | `backend/tests/retrieval_integration.rs` |
| Code-index auth, freshness, navigation, structural search, and fallback | `backend/tests/code_intel_integration.rs` and code-intelligence unit tests |
| Full browser terminal and deterministic mock-agent round trip | `frontend/e2e/06-mock-terminal.spec.ts`, `frontend/e2e/07-agent-roundtrip.spec.ts` |

No source-text assertion is an acceptable substitute for these behaviors.
During extraction, add split-runtime tests before switching the production
route; keep the local/standalone path until the corresponding remote behavior
passes.

### Baseline execution

Baseline captured on 2026-07-27 before runtime extraction:

- Rust clippy passed.
- Rust formatting passed.
- 161 Rust library/binary unit tests and Rust doc tests passed.
- Frontend ESLint and TypeScript checks passed.
- 41 frontend test files with 251 tests passed.
- All 92 registered backend integration tests passed, including PTY,
  WebSocket, ingest, workspace, repository, retrieval, code-intelligence,
  device, database, and correlation targets.
- The repository-wide `make ci` command stopped at the pre-existing structure
  lint because untouched `src/metrics.rs::flow_metrics` spans 206 lines against
  the 140-line limit.
- The real-stack Playwright command did not reach a browser test result:
  Playwright's 300-second web-server deadline expired while the constrained
  runner was still building `sulion-e2e-backend:local-runtime`.

The two incomplete gates are baseline constraints, not accepted substitutes
for later phase verification. Re-run E2E after the runtime image is available,
and do not attribute either failure to the split without new evidence.

## Required split-runtime tests

- Control restart with an active node PTY preserves the process and reconnect
  snapshot.
- Duplicate spawn/worktree/delete operations return one durable result.
- Same-boot reconnect and new-boot reconciliation produce distinct states.
- Node heartbeat expiry leaves history available and rejects mutations.
- A slow or dropped browser attachment does not stall PTY output.
- Network loss during a partial JSONL line commits nothing; recovery ingests
  the completed line once.
- Control cannot read a repo or transcript path directly.
- Node filesystem operations reject traversal and symlink escapes.
- Incompatible protocol peers report a stable actionable error.
- Standalone uses the same protocol and passes the existing E2E suite.

## Deployment drain

Control may request:

- `accepting` — normal operation;
- `draining` — reject new sessions/workspaces while existing PTYs continue;
- `ready` — no live PTYs and safe for node replacement.

The node reports counts and exact live IDs. The deployer consumes node status;
it does not scrape Docker or infer safety from browser connections. Force
replacement is an explicit operator action and records the affected sessions.
