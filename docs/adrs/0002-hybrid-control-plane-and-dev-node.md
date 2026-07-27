# ADR 0002: Hybrid Control Plane and Development Node

## Status

Accepted, 2026-07-27.

## Context

Sulion currently runs as one TrueNAS-hosted Compose stack. The backend owns
both durable application coordination and machine-local development state:
REST, WebSockets, migrations, PTYs, shadow terminal emulators, repository and
worktree operations, transcript ingestion, and background sampling all share
one process.

That shape made sense while the same TrueNAS host owned the application,
repository datasets, and Docker daemon. It no longer fits the intended
deployment:

- repositories and builds need local storage on a dedicated development host;
- LAN macOS and Windows clients still need an SMB view of those repositories;
- PTYs need an unrestricted, ordinary Docker API for development stacks such
  as local Supabase;
- Postgres, embeddings, public ingress, and durable services should remain on
  TrueNAS;
- a control-plane deployment should not terminate active PTYs; and
- Sulion must remain deployable on TrueNAS and conventional Linux without
  maintaining a NixOS-only application implementation.

The dedicated host will run NixOS, and its complete configuration belongs in
this repository. NixOS nevertheless cannot become the only release format:
Sulion's existing OCI images and Compose deployment remain useful portability
and recovery boundaries.

## Decision

Sulion will support three topology roles:

- `control-plane` — durable browser/API/auth/session coordination, migrations,
  broker, and retrieval;
- `dev-node` — PTYs, terminal state, repositories, worktrees, transcript
  ingestion, code intelligence, Samba, and development Docker; and
- `standalone` — both roles on one host, communicating through the same node
  protocol used by a split deployment.

Docker policy is independent of topology:

- `direct` gives PTYs the real CLI and a rootless development daemon;
- `brokered` retains the constrained runner for a shared host.

The user's production target is:

- TrueNAS runs the control-plane role, Postgres, embeddings, public ingress,
  and asynchronous backup storage.
- The dedicated NixOS host runs the dev-node role with local `/home/dev`.
- The node establishes an authenticated outbound connection to the control
  plane. The browser does not connect directly to a node.
- The control plane authorizes and proxies typed terminal and filesystem
  operations. It never mounts source or transcript paths.
- The node owns PTY handles and shadow emulators. A control restart therefore
  drops attachments, not shells.
- The sole JSONL ingester runs on the node because transcript files are local
  there. It preserves `(session_uuid, byte_offset)` idempotency.
- Code intelligence runs on the node because source reads and language
  servers require local repository access.
- The secret broker and master key remain on TrueNAS. The node receives only
  the existing narrowly scoped registration and per-PTY redemption material.

The portable application release remains immutable OCI images plus a common
Compose graph and role overlays. The repository also contains a first-class
NixOS flake, reusable host modules, the user's dedicated host configuration,
and a NixOS VM test. NixOS configures the host facilities and launches the same
dev-node application artifacts used on conventional Linux.

## Node boundary

The control-to-node surface is a typed, versioned protocol rather than a
general remote-command channel. It covers:

- node registration, heartbeat, boot identity, capabilities, and release;
- idempotent session and workspace lifecycle operations;
- terminal snapshot, output, input, resize, and reconnect streams;
- repository, file, Git, upload, and worktree operations;
- agent-session correlation and activity events;
- node-local stats; and
- deployment drain state.

The first deployment supports one active node, but durable rows carry
`node_id`. Control and node releases advertise a compatibility range and must
support rolling deployment in either order within that range.

## Storage and Docker boundary

The dedicated host uses a real `/home/dev` path both on the host and inside the
node workbench. This preserves Docker bind-path and loopback semantics.

Two daemons separate application control from agent-controlled workloads:

- system Docker runs Sulion node-side service containers;
- a rootless daemon owned by `dev` runs development containers.

The node workbench sees only the rootless socket. PTYs cannot manage Sulion's
own containers, the deployer, or the broker.

The host exports `/home/dev/repos` with Samba using one stable Unix/Samba
identity, POSIX ACL and extended-attribute mapping, Windows ACL storage, and
macOS metadata support. Workspaces, agent state, Docker state, and secrets are
not shared.

## Deployment boundary

One CI run builds and tests component images, validates every Compose role,
builds the NixOS configuration and VM test, and publishes a release manifest
containing immutable digests and protocol compatibility.

Deployment consumers are replaceable:

- Komodo may continue to apply the TrueNAS role.
- A root-owned pull deployer applies the NixOS dev-node role.
- Conventional systemd may run the same deployer and Compose bundle.

The NixOS deployer never activates an editable checkout from `/home/dev`.
Node updates drain or wait for active PTYs. Control-plane updates may proceed
without terminating node-owned PTYs.

## Consequences

Positive:

- build, repository, Docker-layer, and language-server I/O stay local;
- TrueNAS retains the stable public and durable service boundary;
- control deployments stop being destructive to active shells;
- the broker key remains isolated from the development environment;
- NixOS configuration is reproducible and tested without forking the
  application packaging;
- standalone remains a supported recovery and integration shape; and
- a future second node is possible without replacing an unnamed singleton
  protocol.

Costs:

- the current backend must be split at a real ownership boundary;
- terminal streaming gains one small LAN hop through the control plane;
- filesystem-backed API routes need typed node equivalents;
- session state must distinguish control disconnect, node disconnect, node
  reboot, process exit, and deletion;
- control and node releases require explicit protocol compatibility; and
- repository migration and Samba identity/ACL preservation need a deliberate
  cutover.

## Alternatives rejected

### Move the entire existing stack to NixOS

This removes the stable TrueNAS control plane, duplicates established ingress
and durable services, and makes the NixOS host an unnecessary availability
dependency for history and secrets management.

### Repackage every service natively in Nix

The backend workbench is a large mutable FHS development environment. Making
Nix the only application packaging format would couple the host migration to a
toolchain rewrite and weaken TrueNAS/generic-Linux portability.

### Keep the backend on the node and move only frontend/broker/retrieval

This is a possible migration bridge, but not the target. The public API would
still own PTYs and filesystem state, node outages would remove the effective
control plane, and control deployments would still terminate shells.

### Mount Dell repositories into TrueNAS

That preserves the existing process layout by reintroducing network filesystem
I/O into builds, which violates the primary storage requirement.

### Give PTYs the system Docker socket

Docker control is equivalent to root on the host. It would collapse the broker,
deployer, and operating-system boundary. A separate rootless development
daemon satisfies the anticipated workload without that authority.

### Connect browsers directly to the node

Direct node connections require another public certificate, route, auth
surface, and reconnect policy. Proxying through the control plane preserves the
single browser origin and lets the node remain outbound-only.

## References

- Implementation plan:
  [`../plans/dedicated-nixos-dev-node.md`](../plans/dedicated-nixos-dev-node.md)
- Node ownership and protocol contract:
  [`../node-protocol.md`](../node-protocol.md)
- Current shipped architecture: [`../architecture.md`](../architecture.md)
- Ingestion ownership: [`../ingestion.md`](../ingestion.md)
- Secret boundary: [`../secrets.md`](../secrets.md)
- Deployment shape: [`../deploy.md`](../deploy.md)
