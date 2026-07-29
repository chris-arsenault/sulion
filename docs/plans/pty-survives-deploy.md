# PTYs survive deployment

## The requirement

1. A node release must not disturb running terminals.
2. A toolset release must not disturb them either — an existing session keeps the
   toolset it started with, new sessions get the current one.
3. Moving a session to a newer toolset is an explicit per-session action, never a
   side effect of a deploy.

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

## Reuse

| Need | Reuse |
| ---- | ----- |
| Unix socket on the shared volume | `correlate.rs` — `UnixListener` on `/run/sulion`, already spoken to from inside PTYs |
| Absent-tolerant framing | `correlate.rs` `SocketMsg` untagged enum with `serde(alias)` / `serde(default)` |
| Launch and reuse a labelled container | `container_runner/postgres.rs` — reuse by `ps --filter label=`, validate labels before adopting |
| PTY internals | `pty/mod.rs` moves into the devenv process largely as-is, including the shadow emulator |

## Phases

Each phase ends green on `make ci` and `make test-rust-integration`.

### Phase 1 — Shells move into a devenv container

A devenv image whose process owns PTY masters and dials the node over the socket.
The node spawns, writes, resizes, and kills through that connection instead of
in-process. One container, current tag only — no versioning yet.

*Exit gate:* recreating the node container leaves shells running; they reattach
with no gap in the shadow emulator; the `node_id IS NULL` orphan sweep stops
firing.

### Phase 2 — Versioned tags

The node holds a current tag, launches a devenv container per tag, and records on
each session which tag it is on. Existing containers are never recreated by a
deploy.

*Exit gate:* rolling the tag leaves existing sessions running on the old
container; new sessions land on the new one. `[depends on Phase 1]`

### Phase 3 — Upgrade, drain, and cleanup

An explicit per-session action moves a session to a container on the current tag.
A container with no sessions left is removed. The in-process PTY path and the
reconciliation that existed only for it come out.

*Exit gate:* the action moves one session and leaves its neighbours alone; empty
non-current containers are reaped; one PTY implementation remains in the tree.
`[depends on Phase 2]`

## Decisions

| # | Decision |
| - | -------- |
| 1 | Does the devenv ship as its own image, or as a tag of `backend`? A separate image stops the toolchain rebuilding on every commit |
| 2 | Does "upgrade this session" restart the shell in place, or start a fresh one resuming the agent transcript? |
| 3 | Reap an emptied container immediately, or after a retention window? |

## Out of scope

- Surviving a host reboot. Processes do not outlive the kernel.
- Migrating a running shell between toolsets without restarting it. Not possible
  for a live process tree; Phase 3 restarts within the session.
