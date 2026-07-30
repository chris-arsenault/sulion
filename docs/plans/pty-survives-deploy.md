# PTYs survive deployment

## The requirement

1. A node release must not disturb running terminals.
2. A toolset release must not disturb them either — an existing session keeps the
   toolset it started with, new sessions get the current one.
3. Moving a session to a newer toolset is an explicit per-session action, never a
   side effect of a deploy.
4. Survival is promised on the dedicated-node paradigm. No other surface gets
   *worse*: the plan changes no PTY lifetime anywhere else. The combined role
   keeps exactly its current behavior — its deploy has always ended its
   in-process PTYs (docs/deploy.md documents this today) — and must keep
   working through every phase.
5. The toolset image rebuilds only when toolset inputs change, never because
   backend code moved.

## The four release surfaces

Impact analysis goes wrong the moment two of these blur. They are separate
mechanisms with separate triggers and separate blast radii.

| # | Surface | Trigger and mechanism | What is recreated | PTY impact today |
| - | ------- | --------------------- | ----------------- | ---------------- |
| 1 | Image build | push to `main` → shared ahara workflow (`.github/workflows/ci.yml` delegates to `chris-arsenault/ahara`) builds every `platform.yml` image, pushes to GHCR at the commit SHA | nothing — a build restarts nothing | none |
| 2 | Control-plane deploy | same push → `deploy-truenas` → Komodo stack on `compose.yaml` | `sulion-backend`, `sulion-frontend`, `sulion-broker`, `sulion-retrieval` on TrueNAS | none in the split topology — the node re-dials `:30081`, browser attachments drop and reconnect |
| 3 | Node deploy | CI advances the `node-release` branch → root-owned `sulion-node-update` timer on `sulion-enclave` runs `sulion-node-deploy.sh`: `compose up -d --remove-orphans` over `compose.yaml` + `deploy/compose.dedicated.yaml` | `sulion-node`, `sulion-ingester`, `sulion-code-intel` | **kills every shell** — the node holds the PTY masters |
| 4 | Combined-role deploy | flip `truenas_compose_path` in `platform.yml` → Komodo deploys `deploy/compose.truenas-standalone.yaml` | the combined backend and its services | ends its PTYs **today** — the loopback node runtime is in-process (docs/deploy.md). This plan leaves that surface's behavior exactly as it is |

Only surface 3 is this plan's target. Surfaces 1 and 2 already behave. Surface 4
must merely keep working. Recreating `sulion-ingester` and `sulion-code-intel` on
surface 3 is harmless: the ingester resumes from `(session_uuid, byte_offset)`
and code-intel holds no session state.

Net PTY-lifetime effect of the whole plan: surface 3 goes from *kills
everything* to *kills nothing* — and no surface gets worse. Standalone
child-process PTYs die with the combined backend exactly as its in-process PTYs
do today: same lifetime, same trigger, no regression. If combined-role survival
is ever wanted, the launch-mode seam already leaves room for it — a
runner-launched devenv container (one bounded runner verb) would outlive the
combined backend the same way devenv containers outlive the node. Out of scope
until asked for.

Two adjacent facts that must stay straight:

- Devenv containers are **not** compose services. If they were, the very next
  node release's `--remove-orphans` would delete them. They are directly
  launched, label-adopted containers, exactly like the dev-postgres containers.
- The `sulion_run` named volume outlives `compose up` recreation (the deploy
  script never runs `down`), so socket paths on it stay stable across node
  releases.
- There is a third PTY-hosting context besides the two deployed paradigms: the
  e2e stack's node runs with `SULION_DOCKER_MODE=none` and no Docker socket
  (`scripts/run-e2e-stack.mjs`). Whatever shape ships must keep working with no
  Docker daemon available to the node process.

## Why it does not work today

The node holds every PTY master fd in its own process (`backend/src/pty/mod.rs:273`
opens it, `:303` holds it). A release runs `compose up -d --remove-orphans`
(`nix/scripts/sulion-node-deploy.sh:70`) and recreates the node container, so the
masters close, slaves get SIGHUP, and the shells die. Boot-id reconciliation then
records the loss — bookkeeping for something that should not have happened.

## Shape

Shells move into **devenv containers**, launched from versioned images.

The node keeps everything it does today except owning PTYs. It launches devenv
containers, holds the current toolset tag, and does the housekeeping. A devenv
container connects back to the node over a unix socket on the shared
`sulion_run` volume and owns the PTY masters for the shells inside it.

Reconnecting is the devenv's job, not the node's. When the node is recreated the
devenv containers keep running, notice the drop, and dial back in. The node does
not have to find them.

A toolset release changes the current tag. The node launches a container from the
new tag for new sessions. The old container is untouched and keeps serving its
shells, because it is simply still running.

There is still one node. Devenv containers are containers the node launches, not
nodes — no pairing, no registration, no TLS. Same host, same trust domain, local
socket, exactly like `correlate.sock` today.

The PTY owner is one binary — the **devenv server** — and where it runs is a
launch mode, not a second implementation:

- **container-direct** — the dedicated node launches it in a devenv container on
  the host Docker daemon. This is the mode that delivers survival.
- **child-process** — where the node process has no Docker daemon
  (`SULION_DOCKER_MODE=none`: the e2e node, integration tests) and in the
  standalone loopback role, the same binary runs as a child process speaking the
  same socket protocol. No survival, by requirement 4 — but one PTY
  implementation and one wire protocol everywhere.

Two version-skew rules follow from requirement 2:

- An old devenv container keeps serving shells against ever-newer nodes, so the
  node↔devenv socket protocol is additive from day one: absent-tolerant fields,
  no version-ordering handshake. Same rule as the node protocol.
- The devenv image carries the toolset only, no per-commit Rust binaries —
  otherwise every backend commit would rebuild it, breaking requirement 5. The
  node delivers the devenv server binary through the shared run volume the
  container already mounts for the socket: it writes a per-release copy there at
  startup, and the container execs that versioned path. A running container
  keeps the binary it launched with; a per-release path is never overwritten in
  place. No host-path assumptions, no second mount.

## Reuse

| Need | Reuse |
| ---- | ----- |
| Unix socket on the shared volume | `correlate.rs` — `UnixListener` on `/run/sulion`, already spoken to from inside PTYs |
| Absent-tolerant framing | `correlate.rs` `SocketMsg` untagged enum with `serde(alias)` / `serde(default)` |
| Launch and reuse a labelled container | `container_runner/postgres.rs` — reuse by `ps --filter label=`, validate labels before adopting |
| PTY internals | `pty/mod.rs` moves into the devenv process largely as-is, including the shadow emulator |

## Phases

Each phase ends green on `make ci` and `make test-rust-integration`.

### Phase 1 — Shells move into a devenv server

The devenv server binary owns the PTY masters and dials the node over the
socket; the node spawns, writes, resizes, and kills through that connection
instead of in-process. Container-direct on the dedicated paradigm (one
container — no versioning yet, and it launches from the existing backend image,
which already carries the toolset and the binary, so this phase needs no new
image and no binary delivery); child-process where the node has no Docker
daemon, which is what keeps the e2e suite and the standalone role green in this
phase. There is no dual hosting period: at this phase's exit every role hosts
PTYs in the devenv server, so e2e and standalone exercise the same protocol the
dedicated paradigm relies on. Step-level expansion:
[pty-survives-deploy-phase1.md](pty-survives-deploy-phase1.md).

*Exit gate:* recreating the node container (surface 3 semantics) leaves shells
running; they reattach with no gap in the shadow emulator; the `node_id IS NULL`
orphan sweep stops firing; the e2e stack passes with its Docker-less node.

### Phase 2 — Versioned tags

The node holds a current tag, launches a devenv container per tag, and records on
each session which tag it is on. Existing containers are never recreated by a
deploy. The devenv image joins `platform.yml` as its own image (surface 1); its
tag advances only when toolset inputs change, so a node release does not roll
devenv containers at all. This is also the phase where binary delivery through
the run volume replaces the backend-image bootstrap from Phase 1.

*Exit gate:* rolling the tag leaves existing sessions running on the old
container; new sessions land on the new one; a backend-only commit does not
roll devenv containers (same image ID, per Decision 5). `[depends on Phase 1]`

### Phase 3 — Upgrade, drain, and cleanup

An explicit per-session action moves a session to a container on the current tag.
A container with no sessions left is removed. The reconciliation bookkeeping
that existed only for node-owned in-process PTYs comes out (the hosting itself
already left in Phase 1); what remains is the devenv server in its two launch
modes.

*Exit gate:* the action moves one session and leaves its neighbours alone; empty
non-current containers are reaped; one PTY implementation remains in the tree;
the standalone role and the e2e stack still pass on the child-process mode.
`[depends on Phase 2]`

## Decisions

| # | Decision |
| - | -------- |
| 1 | **Resolved:** the devenv ships as its own toolset-only image, listed in `platform.yml`. A separate image is what stops the toolchain rebuilding on every commit (requirement 5) |
| 2 | **Resolved:** restart in place — the session keeps its identity and workspace binding; a fresh default shell starts on the current toolset in the same working directory. No agent-specific logic; resuming an agent is the existing resume flow's job |
| 3 | **Resolved:** reap immediately — new sessions and upgrades only ever land on the current devenv, so an emptied non-current container can never refill; a retention window would preserve nothing. Stopped non-current containers are reaped regardless |
| 4 | **Resolved:** the node delivers its per-release devenv server binary via the shared run volume; the container execs the versioned path. A thin per-commit binary layer was rejected because it mints a devenv tag per commit, rolling containers on every release and defeating requirement 5 in spirit |
| 5 | **Resolved:** the shared workflow now supports opt-in `content_addressed_images` in `platform.yml` (change staged in `~/repos/ahara`, uncommitted). Listed images are tagged with the git tree hash of their component directory (`:t-<tree>`); when that tag already exists in GHCR the build is skipped and `:$SHA`/`:latest` are re-pointed at the existing image. Unchanged toolset ⇒ provably identical image content — not merely cache-probably, which matters because a cache-evicted rebuild is not bit-reproducible and would otherwise mint a spurious new image ID. Phase 2 lists `devenv` there; the node rolls containers only when the pulled `:$SHA` image ID differs from the running container's, which under this scheme happens exactly when the toolset tree changed |

## Out of scope

- Surviving a host reboot. Processes do not outlive the kernel.
- Migrating a running shell between toolsets without restarting it. Not possible
  for a live process tree; Phase 3 restarts within the session.
- PTY survival on the standalone / combined role (requirement 4). Its PTYs live
  in the child-process mode and end with the combined backend.
- Slimming the backend image. It stays the workbench for the combined role and
  keeps shipping the `sulion` / `sulion-node` / `sulion-ingester` binaries; only
  the dedicated paradigm's PTYs stop depending on it.
