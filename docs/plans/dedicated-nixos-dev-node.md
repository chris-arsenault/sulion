# Dedicated NixOS development node and TrueNAS control plane

Status: proposed; architecture agreed, implementation not started.

## Outcome

Run Sulion as a durable control plane on TrueNAS and a machine-local
development plane on a dedicated x86_64 NixOS host. Repositories, workspaces,
agent processes, Docker workloads, and builds stay on the dedicated host.
TrueNAS continues to own the public entry point, durable application services,
Postgres, embeddings, and backups.

The repository must continue to support:

- the existing TrueNAS deployment;
- a conventional non-NixOS Linux deployment;
- an all-in-one deployment for recovery and integration testing; and
- the user's complete, opinionated NixOS host configuration.

OCI images and the Compose application graph remain the portable release
contract. NixOS is a first-class host adapter and checked-in appliance
configuration, not Sulion's only packaging format.

## Requirements

### Local development path

- `/home/dev`, canonical repositories, isolated workspaces, package caches,
  Docker state, and build outputs live on local storage on the development
  node.
- No build or repository path may depend on an NFS or SMB mount from TrueNAS.
- `/home/dev/repos` is exported from the development node over SMB3 for
  authenticated macOS and Windows clients.
- The installation is single-user, but Unix ownership, POSIX ACLs, Windows
  ACLs, inheritance, DOS attributes, and macOS metadata must remain coherent.
- Backups to TrueNAS are asynchronous and never sit in the build I/O path.

### Docker

- PTYs on a dedicated host receive the real Docker CLI, Compose plugin, and
  direct access to a Docker-compatible API.
- Sulion does not filter ordinary Docker subcommands or flags in dedicated
  mode.
- The anticipated acceptance ceiling is the complete Scuba Sense local
  Supabase stack, including its full optional service set.
- Development containers run in a rootless daemon owned by `dev`. Rootful-only
  workloads such as device passthrough, kernel module manipulation, and
  privileged Docker-in-Docker are not acceptance requirements.
- Sulion control-plane containers use a separate system Docker daemon. PTYs
  must not receive that daemon's socket.

### Portability

- Application code does not branch on NixOS versus TrueNAS.
- Topology roles and Docker policy are independent:
  - topology role: `control-plane`, `dev-node`, or `standalone`;
  - Docker policy: `direct` or `brokered`.
- A generic Linux host can run the same dedicated or standalone role using
  systemd and Compose.
- TrueNAS can retain the brokered Docker policy while using the same
  application images and common Compose definitions.
- Common service definitions, release metadata, health checks, and environment
  contracts are written once.

### Operational behavior

- A control-plane deployment may disconnect browsers but must not terminate
  PTYs.
- A development-node deployment remains gated while PTYs are active until PTY
  ownership itself can survive a node restart.
- The control plane reports a disconnected node honestly and does not mark all
  sessions dead merely because the control process restarted.
- If TrueNAS or the network is temporarily unavailable, existing PTYs,
  editors, builds, and development containers continue running locally.
- Transcript ingestion catches up idempotently when Postgres becomes
  available again.
- The broker master key never reaches the development node or a PTY.

## Invariants

The existing architecture invariants remain binding:

1. Only an ingester reads Claude or Codex JSONL.
2. REST and browser event paths query Postgres rather than transcript files.
3. The browser terminal remains outside React reconciliation.
4. The node feeds the shadow terminal emulator continuously, with or without
   attached browsers.
5. Transcript idempotency remains `(session_uuid, byte_offset)`.
6. `parent_session_uuid` remains nullable and preserved.

The split adds these invariants:

7. Only the development node owns PTY handles and machine-local process state.
8. The control plane never mounts repository, workspace, or transcript roots.
9. The browser never receives a general-purpose direct node credential.
10. Control-to-node operations are typed, authenticated, idempotent, and
    auditable; there is no generic remote-shell RPC.
11. Database migrations have one owner: the control plane or an explicit
    migration job, never the node and control plane concurrently.
12. The rootless development Docker socket is never mounted into the broker,
    control API, retrieval service, or deployer.

## Target topology

```text
browser
  |
  v
TrueNAS
  frontend / public ingress
  control API
  broker
  retrieval
  Postgres
  embedding service
  |
  | authenticated, outbound-established node channel
  v
dedicated NixOS host
  node runtime
    PTY processes
    shadow terminal emulators
    correlation/activity socket
    Git and worktree operations
  ingester
    Claude and Codex transcript roots
    canonical event and timeline writes
  code-intelligence service
    local read-only repository/workspace access
  system Docker
    node, ingester, and code-intelligence containers
  rootless dev Docker
    agent-created development containers
  Samba
    /home/dev/repos
```

Terminal bytes and typed commands cross the network. Repository contents,
compiler traffic, package caches, and Docker layers do not.

## Deployment roles

### `control-plane`

Runs:

- frontend and same-origin reverse proxy;
- public REST and WebSocket API;
- session, plan, activity, and node catalogue;
- WebSocket ticket issuance;
- database migrations and central maintenance;
- broker; and
- retrieval and semantic indexing.

It owns durable coordination but no PTY, worktree, repository, code-indexing,
or transcript filesystem.

### `dev-node`

Runs:

- `sulion-node`, based on the existing workbench image;
- one ingester worker for both Claude and Codex;
- code intelligence;
- rootless Docker for PTYs;
- local repository and workspace storage;
- Samba; and
- the node-side release agent.

The node image retains the mutable FHS workbench and agent toolchain. The
control image can become a smaller runtime image after the split.

### `standalone`

Runs the control-plane and dev-node roles together. The processes still
communicate through the node protocol, using a loopback connection. This is the
recovery, local integration-test, and portable single-host shape; it is not a
second implementation.

The Docker policy is selected independently:

- `direct` exposes the real rootless daemon to PTYs;
- `brokered` keeps the current constrained runner boundary.

## Runtime ownership

### Control-plane ownership

- Cognito validation and browser/device authentication.
- Public API routing and response models.
- WebSocket tickets and browser session authorization.
- Node registry, health, protocol compatibility, and desired assignment.
- Durable session, plan, activity, future-prompt, and timeline queries.
- Main database migrations.
- Projection/backfill coordination that only requires Postgres.
- Library content on durable control-plane storage.
- Secret bundle and grant management through the separate broker.

### Development-node ownership

- `PtyManager`, PTY reader/writer/resize tasks, process supervision, and shadow
  emulator state.
- Session launch environment and agent CLI wrappers.
- The correlation/activity Unix socket.
- Repository discovery, clone/import/rename/delete, file access, uploads, Git
  operations, and isolated worktree lifecycle.
- Local repo and workspace status sampling.
- Transcript polling and the only reads of Claude/Codex JSONL.
- Code-intelligence discovery, source reads, language servers, and indexing.
- Development-server ports.
- Direct rootless Docker access.

Control API filesystem routes become typed node operations. They do not proxy
arbitrary paths: requests continue to resolve through registered repository or
workspace identities and the node enforces its allowed roots.

## Node protocol

The node establishes the connection outbound so the development host does not
need a public listener. Transport selection is an implementation decision in
the protocol phase, but it must support one long-lived authenticated channel
with multiplexed typed requests and binary terminal streams.

Every envelope carries:

- protocol version;
- node ID and node boot ID;
- request or stream ID;
- optional PTY/workspace identity;
- monotonic sequence where ordering matters; and
- an explicit result or error code.

Required protocol surfaces:

- node registration, heartbeat, capabilities, and observed release;
- desired-session reconciliation;
- spawn, delete, input, resize, interrupt, prompt, and metadata operations;
- attach, terminal snapshot, live output, detach, and reconnect;
- repository, file, Git, upload, and worktree operations;
- agent-session correlation and activity events;
- node-local stats and Docker capability reporting; and
- graceful drain for deployments.

Commands that mutate state use stable idempotency keys. A reconnect may replay
an incomplete command without creating a second PTY or worktree.

The first production version supports one active development node, but the
schema and protocol carry `node_id` so this physical boundary is not encoded as
an unnamed singleton.

## Database changes

Use expand-and-contract migrations so the current combined backend remains
deployable during the transition.

Add:

- a `dev_nodes` table with stable identity, display name, protocol version,
  capabilities, boot ID, heartbeat, connection state, desired release, and
  observed release;
- `node_id` on PTY sessions and workspaces;
- node ownership on code-intelligence roots;
- durable node-operation records for idempotency, status, result, and audit;
- node-aware runtime timestamps that distinguish control disconnect, node
  disconnect, process exit, and deletion.

Do not let a control-plane restart run today's blanket orphan reconciliation.
The node is authoritative for its process inventory and reconciles live session
IDs after reconnect. A changed node boot ID means prior PTYs from that node can
no longer be live and may be marked lost/dead after reconciliation.

Keep repo names globally unique for the initial single-node system. Multi-node
repo scheduling and duplicate repo names are out of scope.

The ingester retains the existing idempotency key. If node-local ingest writes
directly to Postgres, provision a purpose-specific credential and keep it out
of the PTY filesystem. If the implementation instead sends complete transcript
records through the control plane, preserve byte offsets and commit progress
only after the central transaction succeeds. Choose one path in the protocol
phase and test network interruption before rollout.

## Terminal streaming

The browser continues to connect only to the control-plane `/ws` endpoint with
the existing short-lived, single-use ticket.

The control plane:

1. authorizes the browser and resolves the owning node;
2. opens or joins a typed attach stream over the node channel;
3. forwards the node-rendered shadow snapshot;
4. bridges output bytes, input bytes, and resize messages; and
5. tears down only the attachment when either WebSocket disconnects.

The node owns the PTY and shadow emulator throughout. A control-plane restart
drops the browser attachment, not the shell. Reconnection obtains a fresh
snapshot from the node and resumes live output.

## Secrets

- The broker and master key stay on TrueNAS.
- The node receives only the broker URL, its backend registration authority,
  and per-PTY private material generated for the existing signed redemption
  protocol.
- PTY grants remain scoped by PTY ID and expiry.
- Node credentials and database credentials are delivered through root-owned
  systemd credentials or equivalent host-secret files, never through the Nix
  store, Compose source, or `/home/dev`.
- Rootless Docker control does not grant access to the system Docker daemon,
  host deployment state, or broker key.

## Local filesystem and Samba

The dedicated host's canonical paths are:

- `/home/dev/repos`;
- `/home/dev/workspaces`;
- `/home/dev/.claude`;
- `/home/dev/.codex`; and
- rootless Docker state owned by `dev`.

The actual host and node-container paths remain identical so Docker bind mounts
resolve correctly in both the PTY and daemon.

The host filesystem must support POSIX ACLs and extended attributes. The Samba
share uses:

- one real Unix `dev` identity with stable UID/GID;
- one matching authenticated Samba identity;
- `acl_xattr` while retaining system ACL mapping;
- `fruit` and `streams_xattr` for macOS metadata;
- stored DOS attributes rather than Unix execute-bit mapping;
- inherited directory ACLs;
- SMB3 only; and
- LAN-scoped firewall exposure.

Do not use `force user`: local processes and SMB clients must agree on the
actual owner. Preserve Samba's local SID, passdb, ACL xattrs, and relevant
state across reinstall and backup.

Only canonical repositories are shared. Workspaces, agent state, Docker state,
deployment state, and secrets are not.

## Docker topology

The dedicated host runs two daemons:

1. System Docker, controlled by root-owned deployment services, runs
   `sulion-node`, the ingester, and code intelligence.
2. Rootless Docker, owned by `dev`, runs anything created from a PTY.

The node container:

- runs as the stable `dev` UID/GID;
- uses host networking so published development ports are visible at the same
  loopback addresses expected by local tooling;
- bind-mounts `/home/dev` at `/home/dev`;
- mounts only the rootless Docker socket;
- uses the real Docker CLI and Compose plugin in direct mode; and
- does not mount the system Docker socket.

The NixOS host may apply a broad user-slice memory/CPU policy to preserve
control-plane responsiveness, but Sulion does not inject per-container
resource limits or reject normal Docker flags in direct mode.

## Portable deployment layout

Target repository layout:

```text
flake.nix
nix/
  modules/
    sulion-node.nix
    sulion-samba.nix
    sulion-deployer.nix
  hosts/
    dedicated/
      default.nix
      hardware-configuration.nix
  tests/
    dev-node-vm.nix
deploy/
  compose.yaml
  compose.control-plane.yaml
  compose.dev-node.yaml
  compose.standalone.yaml
  compose.truenas.yaml
  release.schema.json
  samba/
    sulion.conf
```

Responsibilities:

- `deploy/compose.yaml` defines common images, health checks, container paths,
  and shared environment.
- Role overlays select services and host integration without copying common
  definitions.
- The TrueNAS override contains dataset paths and Komodo-specific exposure.
- The NixOS module installs host prerequisites and invokes the same dev-node
  Compose role; it does not reproduce the application service graph in Nix.
- The checked-in `nix/hosts/dedicated` configuration is the user's actual host
  policy, not a generic example.
- The generated `hardware-configuration.nix` is the only machine-specific
  hardware leaf. Sulion modules do not branch on CPU model, core count, disk
  vendor, or motherboard.
- A generic Linux deployment uses the same Compose bundle, Samba fragment, and
  release agent through conventional systemd units.

## CI/CD

One CI run:

1. runs Rust, frontend, integration, and E2E gates;
2. builds each OCI image once;
3. validates every Compose role with `docker compose config`;
4. builds and tests the NixOS development-node configuration in a VM;
5. runs a generic-Linux standalone smoke test;
6. publishes images by immutable digest; and
7. publishes a signed release manifest containing Git SHA, schema version,
   control protocol range, node protocol version, and component digests.

Deployment consumers:

- Komodo applies the TrueNAS control-plane or standalone role.
- A root-owned pull deployer on the NixOS node applies the dev-node role.
- Generic Linux runs the same deployer through systemd.

The deployer never deploys from the editable `/home/dev/repos/sulion` checkout.
It consumes a verified release manifest and immutable Compose bundle.

Control and node components roll independently within an advertised
compatibility window. The control plane must support the currently deployed
node before it advertises or requests a node upgrade.

Node deployment sequence:

1. fetch and verify the release;
2. validate Compose and protocol compatibility;
3. request node drain;
4. wait while live PTYs exist unless an explicit force action is supplied;
5. retain the previous manifest and image digests;
6. apply the new node services;
7. require registration, health, Docker capability, and filesystem checks; and
8. roll back to the previous component digests on failed activation.

Database migrations are not automatically reversed. They must be backward
compatible across the rollback window.

## Failure behavior

### Control-plane restart

- Active PTYs continue.
- Browser terminal attachments disconnect and reconnect.
- The node reconnects and reconciles its inventory.
- No session becomes orphaned solely because the control process restarted.

### TrueNAS or network outage

- Active PTYs, editors, Samba, builds, and development containers continue.
- New UI actions, history queries, retrieval, and secret redemption may fail.
- Transcript files remain local and complete.
- Ingestion retries from the last committed byte offset after recovery.

### Node restart

- PTYs owned by the prior node boot cannot be recovered initially.
- The node reports a new boot ID.
- Control reconciliation marks unrecoverable sessions lost/dead with an
  explicit reason.
- Repository and transcript state remain local and are rediscovered.

### Duplicate or delayed command

- Stable operation IDs prevent duplicate PTYs, worktrees, prompts, or deletes.
- The control plane can query the durable operation result after reconnect.

### Version incompatibility

- The node remains connected for status but refuses unsupported mutations.
- The UI reports the required control/node upgrade instead of attempting a
  partially compatible operation.

## Migration and rollback

### Pre-cutover

- Back up the main and broker databases.
- Snapshot existing TrueNAS Sulion datasets.
- Provision the NixOS host and validate it without making it authoritative.
- Register the node and verify the authenticated channel.
- Run direct Docker and full Supabase acceptance tests.
- Run Samba ACL tests from Linux plus real macOS and Windows clients.
- Perform an initial repository/workspace copy while the old system remains
  live, preserving ownership, ACLs, xattrs, symlinks, and Git metadata.

### Cutover

1. Stop creation of new PTYs and filesystem mutations.
2. Drain or explicitly terminate remaining old-host PTYs.
3. Run a final incremental copy.
4. Compare repository inventory, Git refs, worktree registrations, dirty state,
   representative file hashes, ownership, ACLs, and xattrs.
5. Make the Dell copy authoritative and the old TrueNAS copy read-only.
6. Deploy the TrueNAS control-plane role.
7. Start and register the NixOS dev-node role.
8. Switch the SMB share and LAN clients to the new host.
9. Run session, terminal reconnect, ingestion, secrets, code-intelligence,
   Docker, and browser smoke tests.

Never leave both repository copies writable.

### Rollback

Before new writes on the NixOS copy, rollback may point services and SMB back
to the preserved TrueNAS snapshot.

After new writes, rollback is not an automatic endpoint switch. Quiesce both
sides, copy changes back with ACL/xattr preservation, verify Git and filesystem
state, and only then restore the old deployment. Database migrations remain
compatible with the old control version throughout the declared rollback
window.

## Implementation phases

### Phase 1 — Contract and regression baseline

Deliverables:

- Architecture decision record for control-plane/node ownership.
- Protocol semantics and failure-state specification.
- Deployment role and Docker-policy contract.
- Inventory of every current backend route/background task and its future
  owner.
- Behavioral regression tests for current PTY, reconnect, ingestion,
  workspace, broker, and code-intelligence contracts.

Exit gate:

- Every current backend responsibility has exactly one target owner.
- Existing TrueNAS behavior and test gates remain unchanged.
- No application deployment has moved.

### Phase 2 — Portable deployment roles

Deliverables:

- Common Compose definition plus dedicated-host, standalone, and TrueNAS
  overlays for the current combined runtime.
- Parameterized paths and service endpoints instead of TrueNAS addresses in
  common definitions.
- Independent `direct` and `brokered` Docker policies.
- CI validation of every Compose combination.
- Existing Komodo deployment expressed through the TrueNAS/standalone profile.

The control-plane and dev-node overlays are added in Phases 5 and 6, once the
services they select exist. Until then, the dedicated-host overlay is a
functional transitional standalone deployment; role files must not imply a
physical split that the binaries do not yet implement.

Exit gate:

- Current TrueNAS deployment renders equivalently from the new files.
- Generic Linux standalone and the transitional dedicated deployment render
  from the common graph.
- Common service definitions are not duplicated across host adapters.

### Phase 3 — First-class NixOS dedicated host

Deliverables:

- Root flake and reusable Sulion modules.
- Checked-in `nix/hosts/dedicated` configuration.
- Stable `dev` and service identities.
- Local `/home/dev` layout.
- System Docker plus rootless development Docker.
- Samba ACL/macOS configuration and persistent Samba identity state.
- Host firewall, secret-credential paths, asynchronous backup timer, and
  root-owned stack/deployer services.
- NixOS VM test.

Exit gate:

- A clean NixOS VM realizes the host contract without manual package or service
  installation.
- PTY-equivalent processes can use unrestricted rootless Docker.
- SMB ownership, inheritance, xattrs, and macOS metadata pass automated
  tests where emulatable.
- The current standalone Sulion stack can run on the host without becoming the
  production authority.

### Phase 4 — Node schema and authenticated protocol

Deliverables:

- Expand-only node registry and ownership migrations.
- Node enrollment and credential lifecycle.
- Outbound node connection with heartbeat and compatibility handshake.
- Durable idempotent node-operation model.
- Control-side node status in app state and UI.
- Standalone loopback transport.

Exit gate:

- Control restarts do not mark simulated node sessions dead.
- Reconnect, duplicate command, heartbeat expiry, new boot ID, and incompatible
  protocol behavior are integration tested.
- The existing local execution path remains available during migration.

### Phase 5 — Extract the development runtime

Deliverables:

- `sulion-node` binary/image owning PTYs, shadow emulation, correlation, and
  local filesystem/worktree operations.
- Separate ingester binary/image owning all JSONL reads.
- Node-local code-intelligence deployment.
- Control-side typed proxies for session, terminal, repo, file, Git, upload,
  and workspace operations.
- Real browser WebSocket-to-node terminal bridging.
- Direct Docker mode in the node workbench.

Exit gate:

- Standalone E2E runs through the node protocol rather than in-process
  `PtyManager`.
- A control restart preserves the PTY and reconstructs the terminal snapshot
  after browser reconnect.
- Network interruption followed by recovery produces no duplicate or missing
  ingested events.
- Neither control nor frontend mounts local source or transcript paths.

### Phase 6 — TrueNAS control-plane deployment

Deliverables:

- TrueNAS control-plane Compose role.
- Control-only frontend, API, broker, and retrieval deployment.
- Removal of runner, repo, workspace, transcript, and code-intelligence mounts
  from the control role.
- Ahara ingress and health checks pointed at the stable control plane.
- Control/node version compatibility surfaced operationally.

Exit gate:

- TrueNAS serves the full UI and history with the node disconnected.
- The UI reports node unavailability and refuses local mutations cleanly.
- Connected-node session, file, Git, terminal, and secret flows pass through
  the production network boundary.
- A control-plane redeploy leaves an active NixOS PTY alive.

### Phase 7 — Release automation and production cutover

Deliverables:

- Signed release manifest and immutable component digests.
- Root-owned portable pull deployer.
- Komodo control-plane deployment and NixOS node deployment from the same
  release.
- Node drain/session gate, health validation, and digest rollback.
- Repository migration tooling and evidence report.
- Cutover and rollback runbook.

Exit gate:

- CI can release without a self-hosted GitHub Actions runner.
- Control and node roll independently within their compatibility window.
- Repository migration validation is clean.
- Only the NixOS repository copy is writable after cutover.

### Phase 8 — Acceptance and documentation

Deliverables:

- Full Scuba Sense Supabase acceptance on the NixOS node.
- Docker CLI/Compose/build/network/volume/interactive test matrix.
- macOS and Windows SMB acceptance record.
- TrueNAS outage, node outage, reconnect, failed deploy, and rollback drills.
- Updated architecture, ingestion, secrets, development, and deployment docs.
- Removal of transitional combined-backend paths after the rollback window.

Exit gate:

- All acceptance criteria below pass.
- The old TrueNAS repository dataset remains only as a documented backup or is
  retired explicitly.
- No duplicated production runtime path remains.

## Acceptance criteria

- Repository and build I/O are local to the NixOS host.
- macOS and Windows authenticate as the single intended user and observe
  stable owners, directories, inheritance, and metadata.
- A local Linux process and an SMB client agree on effective repository access.
- Standard Docker and Compose commands are not rejected by Sulion.
- Scuba Sense's trimmed and full Supabase stacks start, run tests, expose
  expected loopback ports, and stop cleanly.
- PTY bind mounts resolve to the same host paths.
- An agent-controlled Docker daemon cannot manage Sulion control containers or
  read the broker key.
- TrueNAS hosts the durable control plane and Postgres without hosting the hot
  repository filesystem.
- A control-plane restart does not terminate PTYs.
- A browser can reconnect and receive an up-to-date shadow snapshot.
- A temporary TrueNAS outage does not stop local PTYs or builds; ingestion
  resumes without duplicate events.
- A node reboot is represented distinctly from a control disconnect.
- TrueNAS, generic Linux, and NixOS deployment roles consume the same OCI
  images and common Compose contract.
- The repository contains and tests the user's dedicated NixOS configuration.
- CI/CD needs no Komodo shell on the NixOS host and never deploys from an
  editable agent checkout.

## Non-goals

- Multiple users, Active Directory, or per-user development nodes.
- General multi-node scheduling or load balancing.
- Moving Postgres or embeddings off TrueNAS.
- Kubernetes or another orchestrator.
- Nix-native repackaging of every Sulion application service.
- Rootful privileged Docker access from PTYs.
- Zero-downtime upgrades of the node process itself.
- Making development servers publicly reachable without a separate,
  authenticated routing design.

## Documentation ownership

During implementation:

- `docs/architecture.md` owns the shipped runtime shape and invariants.
- `docs/ingestion.md` owns live-ingest and projection/backfill ownership.
- `docs/secrets.md` owns broker/node credential boundaries.
- `docs/deploy.md` owns supported roles, release flow, cutover, and rollback.
- `docs/development.md` owns CI and local verification commands.
- This file owns the unfinished implementation sequence and is retired or
  converted to a historical decision record when every phase is complete.
