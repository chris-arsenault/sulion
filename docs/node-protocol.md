# Development-node contract

Sulion has one application and two production roles:

- the TrueNAS control plane owns the browser API, authentication, durable
  coordination, migrations, broker, retrieval, and Postgres-backed reads;
- `sulion-enclave` owns PTYs, repositories, workspaces, transcript ingestion,
  code intelligence, and development Docker.

The portable standalone role runs both sides in one process through an
in-memory connection. It uses the same request and terminal implementations as
the split deployment, so NixOS is a host adapter rather than an application
fork.

## Stability choices

The production target has one configured node. The protocol deliberately does
not implement a multi-node scheduler, capability negotiation, rolling protocol
ranges, release reconciliation, durable remote-operation ledger, replay cache,
credential history, drain controller, or remote shell.

There is one protocol version and three persisted connection states:
`enrolled`, `connected`, and `disconnected`. A mismatched or unauthenticated
peer is rejected; it is not retained as another lifecycle state. Upgrades that
change the protocol must update control and node together.

This leaves four mechanisms:

1. one-time enrollment and an Ed25519 node identity;
2. an outbound authenticated WebSocket;
3. direct typed request/response messages and terminal streams; and
4. a heartbeat containing boot identity and live PTY inventory.

## Ownership

A responsibility belongs on the node when it requires a process handle,
repository or workspace bytes, transcript bytes, or Docker. It belongs on
control when it is browser authentication, durable coordination, a
Postgres-only query, or a shared service.

Control never mounts the dedicated host's source tree. The node never receives
the broker master key, browser JWTs, or arbitrary SQL access beyond its
existing application database role.

The node owns:

- PTY spawn, supervision, input, resize, output, and the continuously fed
  shadow terminal;
- repository, worktree, file, Git, upload, and agent-process operations;
- the only Claude and Codex JSONL readers;
- code discovery and language-server processes; and
- direct access to the dedicated host's Docker daemon.

Control owns:

- `/api` authorization and durable resource records;
- `/ws/sessions/:id` browser authorization and bridging;
- history, timeline, plan, metrics, and other Postgres-backed reads;
- database migrations and Postgres-only repair;
- the secret broker and retrieval service; and
- the current node connection reference.

## Enrollment and connection

The checked-in host configuration fixes the node ID. An authenticated operator
creates a short-lived token for that ID:

```text
POST /api/nodes/enrollment-tokens
{ display_name, target_node_id, ttl_seconds? }
```

The node consumes it once:

```text
POST /api/nodes/enroll
{ token, public_key }
```

Only the token hash and the current public key are stored. Re-enrollment for
the same configured ID replaces that key and closes its active connection.
There is no credential-generation or revocation-history subsystem. Removing a
compromised node is an operator action: stop it, replace the key through a new
targeted enrollment token, and restart it.

The long-lived endpoint is `GET /ws/nodes`. Control sends a random challenge.
The node signs the challenge, stable node ID, fresh boot ID, and exact protocol
version. Control verifies the enrolled public key before accepting messages.
Production transport uses TLS at
`wss://sulion.services.ahara.io/ws/nodes`.

## Wire format

Every post-handshake message uses this JSON envelope:

```text
protocol_version
node_id
boot_id
message_id
message_kind
request_id?
session_id?
workspace_id?
stream_id?
sequence?
payload
```

`request` messages contain a closed request kind and structured payload. The
node replies once with `request.result`, either a structured result or a stable
error code and message. There is no generic executable, host path, PID,
signal, Docker command, or shell-fragment request.

Large file responses and uploads use `protocol.fragment` envelopes because a
single WebSocket frame is capped at 256 KiB. Fragment count, concurrent groups,
and total reassembled bytes are bounded.

Unknown message kinds are ignored. A malformed authenticated message closes
that connection. A random per-connection ID prevents a delayed close from an
old socket from overwriting a successful reconnect.

## Terminal streams

Control authorizes a browser ticket, resolves the session's node, and opens a
typed attachment. The node sends a shadow-terminal snapshot before live
output. Input, resize, and detach messages identify the session and stream.

Detaching a browser does not terminate its PTY. Per-attachment channels are
bounded; a slow client may lose the attachment and reconnect, but it cannot
stall the PTY reader or shadow emulator.

## Heartbeat and reboot behavior

The heartbeat reports the node's current boot ID and complete live PTY ID
inventory.

- A control restart does not mark node-owned sessions dead. The node reconnects
  with the same boot ID and reports the surviving PTYs.
- A socket loss marks the node disconnected and records disconnect timestamps
  on its live PTYs, but does not claim that those processes died.
- A different boot ID proves that processes from the previous boot cannot
  survive. Only those prior-boot PTY rows are ended with `node_reboot`.
- A missing node makes mutations return `503`; Postgres-backed history remains
  available.

The legacy startup orphan pass only touches rows without a `node_id`.

## Standalone portability

`SULION_NODE_TRANSPORT=loopback` creates one internal node and connects the
same `NodeRuntime` directly in memory. `SULION_NODE_TRANSPORT=remote`, selected
by the control-plane role, disables local filesystem and PTY fallback.

Standalone deployments therefore keep the existing Compose portability
without duplicating node behavior or depending on NixOS.

## Required behavioral checks

The focused node tests cover:

- control readiness and mutation refusal while the node is absent;
- targeted enrollment and authenticated connection;
- same-boot reconnect versus new-boot session termination;
- the shared direct request path in loopback mode;
- PTY survival across control replacement; and
- traversal and symlink-escape rejection.

The existing PTY, WebSocket, ingestion, workspace, repository, and browser
suites remain the behavioral contract. Source-text assertions are not a
substitute.
