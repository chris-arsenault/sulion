# Dedicated NixOS development node

Status: Phases 1-6 implemented; Phase 7 is being simplified before cutover.

## Outcome

Sulion keeps its durable control plane on TrueNAS and moves machine-local
development work to `sulion-enclave`.

- Repositories, workspaces, PTYs, builds, Docker layers, transcripts, and code
  intelligence are local to the NixOS machine.
- Postgres, public ingress, authentication, broker, retrieval, and the browser
  control API remain on TrueNAS.
- `/home/sulion/repos` is an authenticated SMB3 share for LAN Windows, macOS,
  and Linux clients.
- The application remains deployable through the same OCI images and Compose
  graph on TrueNAS, NixOS, or conventional Linux.
- The repository contains the complete dedicated NixOS configuration.

NixOS configures the host. It does not become a second implementation of
Sulion.

## Requirements

1. Build and repository I/O never crosses a TrueNAS filesystem mount.
2. The host uses the stable single-user identity `sulion`, UID/GID 7321.
3. Samba preserves actual Unix ownership, inherited POSIX ACLs, Windows ACL
   xattrs and DOS attributes, and macOS metadata.
4. Dedicated-host PTYs receive an ordinary Docker CLI, Compose, BuildKit, and
   the system Docker socket. Sulion does not filter standard options.
5. The expected maximum workload is the complete Scuba Sense local Supabase
   stack.
6. A TrueNAS control redeploy may detach browsers but must not terminate
   node-owned PTYs.
7. A node reboot is distinct from a network or control disconnect.
8. Only the ingester reads Claude or Codex JSONL; its byte-offset transaction
   contract remains unchanged.
9. The broker master key never reaches the development node.
10. Node releases come from CI-built OCI images, not an editable checkout.

## Deliberate non-features

The first production system does not contain:

- multi-node scheduling or a generalized node registry;
- capability or protocol-range negotiation;
- desired/observed release reconciliation;
- a durable remote-operation ledger, replay cache, or idempotency framework;
- credential-generation, rotation-history, or revocation-history tables;
- signed release manifests in addition to existing Git/GHCR identity;
- automatic drain orchestration or automatic rollback;
- NixOS-native repackaging of the application containers;
- service discovery daemons, node-local backup orchestration, or a second
  Docker daemon.

These mechanisms may be added only in response to an observed need. They are
not prerequisites for one dedicated machine.

## Topology

```text
browser
   |
   v
TrueNAS
  frontend + control API
  auth + broker + retrieval
  Postgres + embeddings
   |
   | outbound authenticated WebSocket
   v
sulion-enclave
  sulion-node
    PTYs + shadow terminals
    repositories + workspaces + Git
    direct system Docker
  sulion-ingester
    local Claude/Codex transcripts
  code-intel
    local source + language servers
  Samba
    /home/sulion/repos
```

Only terminal bytes and typed requests cross the node channel. Source,
compiler traffic, package caches, and Docker layers remain local.

## Runtime shapes

The common Compose graph supports:

- `control-plane`: TrueNAS frontend, API, broker, and retrieval;
- `dev-node`: node, ingester, and code intelligence;
- `standalone`: both responsibilities on one conventional Linux host.

`direct` Docker is for the single-user dedicated host. `brokered` Docker keeps
the constrained runner for shared-host standalone deployments.

Standalone uses the same `NodeRuntime` request and terminal code through an
in-memory connection. There is no NixOS application fork and no local fallback
inside the control-only role.

## Host contract

The checked-in host configuration declares:

- x86_64 UEFI boot with GPT, a 1 GiB ESP, and one ext4 root filesystem;
- hostname `sulion-enclave`;
- user/group `sulion` at 7321:7321;
- system Docker, with `sulion` in its fixed-GID Docker group;
- NetworkManager, wired DHCP, LAN-only SSH, and optional Wi-Fi;
- `/home/sulion/repos` and `/home/sulion/workspaces`;
- Samba on TCP 445 only, with SMB3 and `acl_xattr fruit streams_xattr`;
- a root-owned node key and runtime environment under `/var/lib/sulion`; and
- a boot-enabled root-owned Compose unit that waits for its runtime files.

The host intentionally has one Docker daemon. Docker authority is equivalent
to host root, which is acceptable for this dedicated single-user machine and
is required for ordinary privileged and nested development stacks.

## Node contract

There is one configured node ID, one exact protocol version, and three states:
`enrolled`, `connected`, and `disconnected`.

Enrollment consumes a short-lived token targeted to that fixed ID and stores
the current Ed25519 public key. The node establishes an outbound TLS
WebSocket, proves the challenge, and reports a fresh boot ID. Re-enrollment
replaces the key and active connection.

Control sends direct typed requests and receives one result. Terminal
attachments are typed streams. There is no generic remote command. Large file
payloads use bounded fragmentation.

Heartbeat inventory provides only the recovery facts that matter:

- same boot after control replacement means PTYs may still be alive;
- disconnection records uncertainty without claiming process death;
- a new boot ends only PTYs owned by the prior boot.

The full contract is in [`../node-protocol.md`](../node-protocol.md).

## Database shape

The split adds only:

- `dev_nodes`, containing the fixed node's current public identity, protocol,
  boot, connection, and heartbeat;
- `dev_node_enrollment_tokens`, containing hashed one-time tokens;
- nullable node ownership on PTYs, repos, workspaces, and code roots; and
- node boot/disconnect facts on PTYs.

Nullable ownership preserves the portable standalone migration seam. The
control plane remains the sole migration owner.

## Deployment

CI continues to test and publish OCI images tagged by Git commit. Komodo applies
the TrueNAS control plane. The NixOS host uses the root-owned Compose unit and
the same commit tag.

The stable first node deployment mechanism is intentionally manual promotion:

1. CI publishes the requested commit's images.
2. The operator confirms that no PTY may be lost.
3. One root-owned command validates the tag, pulls the three node-side images,
   renders Compose, and applies them.
4. The operator checks the node connection and starts a disposable PTY.

This is continuous delivery with an explicit node activation gate. It avoids a
timer unexpectedly killing a live shell and avoids inventing a distributed
deployment controller. The TrueNAS control plane can continue to deploy
automatically because replacing it does not own the PTYs.

Rollback is the same explicit command with the prior known-good commit tag.
Database migrations are not rolled back; they remain backward compatible.

## Repository migration

The migration is an operator-controlled `rsync` cutover, not a synchronization
service.

1. Keep TrueNAS authoritative and run an initial
   `rsync -aHAX --numeric-ids` to `/home/sulion/repos`.
2. Compare repository names, Git refs, representative file hashes, owners,
   ACLs, and xattrs.
3. Stop new sessions and repository writes on the old host.
4. Run the final incremental copy and repeat the comparison.
5. make the TrueNAS copy read-only.
6. Start the NixOS node role and point SMB clients at `sulion-enclave`.
7. Verify browser terminals, Git, ingestion, code intelligence, secrets,
   Docker, and SMB.

Both copies must never be writable at the same time. Before new NixOS writes,
rollback can restore the old role directly. After new writes, rollback requires
another quiesced verified copy back to TrueNAS.

## Phases

### Phase 1 — Architecture and baseline

Status: complete.

- Defined control/node ownership and retained the existing invariants.
- Captured the portable deployment requirement.
- Established the behavioral regression baseline.

### Phase 2 — Portable deployment roles

Status: complete.

- One Compose graph with control, node, standalone, and TrueNAS overlays.
- Independent direct and brokered Docker policies.
- No duplicated application definition for NixOS.

### Phase 3 — Dedicated NixOS host

Status: complete.

- Flake, Disko installer, checked-in host, Samba, SSH-key bootstrap, Docker,
  firewall, filesystem layout, and VM contract.
- Hostname `sulion-enclave`; primary identity `sulion`.

### Phase 4 — Node connection

Status: complete, then simplified.

- Fixed-ID enrollment and Ed25519 challenge authentication.
- Exact-version outbound WebSocket, heartbeat, boot identity, and app-state
  status.
- Standalone in-memory connection.
- Removed capabilities, release state, credential history, operation ledger,
  replay, quarantine, and drain state.

### Phase 5 — Development runtime extraction

Status: complete.

- `sulion-node` owns PTYs and local filesystem/Git work.
- `sulion-ingester` owns all JSONL reads.
- Code intelligence runs beside local source.
- Control routes and browser terminal bridging use typed node messages.

### Phase 6 — TrueNAS control plane

Status: complete in source; live cutover remains.

- TrueNAS role contains no repository, workspace, transcript, node, runner, or
  Docker mount.
- Durable reads remain available while the node is disconnected.
- Node-owned mutations return `503` instead of falling back locally.

### Phase 7 — Minimal delivery and cutover

Status: in progress.

- Remove the abandoned generalized control and optional host subsystems.
- Add one root-owned commit-tag deployment command.
- Add the initial/final repository copy and comparison commands.
- Document explicit activation, rollback, and cutover.
- Do not add automatic drain, release manifests, or an updater control plane.

Exit gate:

- the node can pull and activate a CI-published commit tag without Komodo;
- the previous tag can be explicitly restored;
- migration comparison is clean; and
- only the NixOS repository copy is writable after cutover.

### Phase 8 — Acceptance and cleanup

Status: pending.

- Run the full Scuba Sense Supabase stack.
- Verify Docker build, Compose, networks, volumes, bind mounts, interactive
  use, published ports, and required privileged options.
- Verify SMB from real Windows and macOS clients.
- Exercise TrueNAS outage, control redeploy, node reboot, and explicit
  rollback.
- Retire the writable TrueNAS repository role only after the rollback window.

## Final acceptance

- Hot development I/O is local.
- Windows and macOS see coherent single-user ownership and metadata.
- Standard Docker and the full expected Supabase stack work without Sulion
  restrictions.
- A control redeploy does not kill node PTYs.
- A browser reconnect receives the current shadow snapshot.
- Ingestion resumes from committed offsets after a TrueNAS outage.
- TrueNAS, generic Linux, and NixOS consume the same images and Compose
  contract.
- The dedicated host can deploy CI-published images without a Komodo shell or
  an editable source checkout.
