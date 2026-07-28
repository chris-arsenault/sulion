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

There is one protocol version and four persisted connection states: `pending`,
`enrolled`, `connected`, and `disconnected`. Only the fixed node ID can enter
the pending state; other unknown or unauthenticated peers are rejected.
Upgrades that change the protocol must update control and node together.

This leaves four mechanisms:

1. one-click pairing approval and an Ed25519 node identity;
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

The checked-in host configuration fixes one dedicated node ID. On first start,
the node creates its Ed25519 key locally and includes the public key in its
signed handshake. Control verifies proof of possession and exposes the
fingerprint as pending in the authenticated UI. The operator approves it with:

```text
POST /api/nodes/{id}/approve
```

Approval stores the submitted public key. The node's existing outbound retry
loop then reconnects without a copied token or a node-side command. A different
key for the same fixed ID becomes a new pending approval and does not replace
the accepted key until approved. There is no credential history, tenant model,
or separate enrollment service.

The long-lived endpoint is `GET /ws/nodes`. Control sends a random challenge.
The node signs the challenge, stable node ID, fresh boot ID, and exact protocol
version. Control verifies the enrolled public key before accepting messages.

Nodes reach `/ws/nodes` on the backend's own LAN-bound port,
`192.168.66.3:30081`, not through the frontend proxy — which returns 404 for
that path — and no upstream registration points at it, so a node can never pair
over the public reverse proxy.

That port is for **enrollment only**. Everything past it runs inside a
WireGuard tunnel terminated in the control process's own network namespace by
the `node-tunnel` sidecar, which holds `NET_ADMIN` so the control plane does
not. Because `wg0` is an interface the control process binds directly, there is
no forwarding or address translation in the path and a node's real tunnel
address is what the source check sees.

The tunnel cannot exist before the two ends know each other's keys, so first
contact is staged:

```text
1st connect   cleartext, 192.168.66.3:30081   node offers its WireGuard public key
              → PairingRequired, operator approves in the UI
2nd connect   cleartext                        control returns the peering; no credentials
              → node writes wg0.conf, host brings the interface up
3rd connect   ws://10.88.0.1:8080/ws/nodes     credentials delivered over the tunnel
thereafter    tunnel only
```

Credentials are refused on any connection that did not arrive from the tunnel
subnet, so they never cross the cleartext hop — that hop carries public keys and
the peering, and nothing else. Approving a node accepts its identity key and its
tunnel key together and allocates its tunnel address, so one press does all of
it and a node cannot rotate its tunnel key unnoticed.

`AllowedIPs` on the node side is control's single address, not the subnet: the
tunnel exists to reach the control plane, and nodes have no business routing to
each other through it. The sidecar reconciles the approved-peer set every few
seconds, so revoking an approval removes the peer rather than leaving a working
interface until the next restart.

Connecting directly is what makes the boundary checkable: the backend sees each
node's real address and refuses any source outside `SULION_NODE_LAN_CIDR`
before issuing a challenge. Docker preserves the source address for off-host
clients, which every node is. A forwarded `X-Real-IP` is honoured only from a
peer inside `SULION_NODE_TRUSTED_PROXY_CIDR`, which is unset in this deployment
and therefore trusts nobody; it exists for a deployment that does front the node
port with a proxy.

The handshake runs both ways. The node signs control's challenge, and control
signs back over that challenge, the node's own nonce, and its identity, in
`control.hello_ack`:

```text
control.hello_ack
  control_proof.public_key   — control's Ed25519 identity
  control_proof.signature    — over challenge, node nonce, node and boot id
```

A node records that key the first time it pairs and requires it on every later
connection. A different key — or a missing proof, which would otherwise be a
free downgrade — is refused outright. First pairing is therefore the one moment
a node can be captured, which is accepted because the machine is being
installed by hand at that point. The pin lives at
`/var/lib/sulion-node/control-key.pub`, root-owned, and survives container
replacement, release upgrades, and `nixos-rebuild`.

Recovering from a legitimately replaced control plane is deliberate:

```bash
sudo rm /var/lib/sulion/node/control-key.pub
sudo systemctl reload sulion-stack.service
```

The control identity itself lives in the `control_identity` table, so it
survives redeploys without a new dataset. Anything able to read it can already
read the credentials control hands out.

Approval is also the node's only bootstrap. A node holds nothing but its
identity key, so once control accepts the handshake it sends
`control.node_config` immediately after `control.hello_ack`:

```text
control.node_config
  digest   — SHA-256 over the canonical key=value rendering
  values   — the forwarded environment map
```

`values` carries a fixed key list (database credentials, retrieval token,
broker registration token). **Both ends enforce that list.** The receiving check
is the one that matters: the file the node writes is consumed as a Compose
`--env-file` and as `EnvironmentFile=` for a root systemd unit, where Compose
interpolates it into image references, bind-mount sources, and the
privilege-drop identity. An unexpected key is refused outright rather than
filtered, because its presence means the peer is not the control plane this
node expects.

`signature` covers the digest and is bound to the connection's node nonce, so
the payload stays tamper-evident even though the channel is not yet encrypted,
and cannot be lifted onto another connection. The node writes the map plus
`SULION_NODE_CONFIG_DIGEST` to root-owned host state and its host activates the
stack around it. A node whose
own environment already carries the delivered digest simply proceeds. Nothing
is copied between machines and no token is handled by hand.

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
- pending-key approval and authenticated connection;
- same-boot reconnect versus new-boot session termination;
- the shared direct request path in loopback mode;
- PTY survival across control replacement; and
- traversal and symlink-escape rejection.

The existing PTY, WebSocket, ingestion, workspace, repository, and browser
suites remain the behavioral contract. Source-text assertions are not a
substitute.
