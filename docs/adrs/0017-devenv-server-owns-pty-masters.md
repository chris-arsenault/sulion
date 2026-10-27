# ADR 0017: Devenv Server Owns PTY Masters

## Status

Accepted, 2026-07-30. Amends ADR-0002: the devenv server, not the node
process, owns PTY handles and shadow emulators.

## Context

ADR 0002 moved PTYs onto the dedicated node so that a control-plane release
would drop browser attachments rather than shells. It left the node process
holding every PTY master. A node release runs `compose up -d
--remove-orphans` and recreates the `sulion-node` container, so the masters
closed, the slaves received SIGHUP, and every shell died. Boot-identity
reconciliation then recorded the loss as orphaned sessions.

The user's requirement was plain: PTYs survive a deployment, and the orphan
reconciliation that existed to clean up afterwards was bookkeeping for a loss
that should not happen. The plan that followed separated four release
surfaces: image build, control-plane deploy, node deploy, and combined-role
deploy. Only the node deploy killed shells. The combined (standalone) role had
always ended its in-process PTYs and was out of scope for survival but had to
keep working through every phase. A toolset release had to leave existing
sessions on the toolset they started with, and the toolset image had to
rebuild only when toolset inputs changed.

## Decision

Shells live in the **devenv server** (`sulion-devenv`,
`backend/src/devenv/`), which owns the PTY masters and the shadow emulators.
The node process owns no PTY masters (architecture invariant 7). The devenv
server dials the node over `/run/sulion/devenv.sock` on the shared
`sulion_run` volume and reconnects on its own after the node is recreated.

Where the server runs is a launch mode, not a second implementation:

- **container-direct** on the dedicated host, as a label-owned container the
  node launches on the host Docker daemon, adopted on later node starts and
  never recreated by a deploy. It is not a compose service.
- **child-process** where the node has no Docker daemon (the e2e stack,
  integration tests) and in the standalone loopback role. Same binary, same
  socket protocol, no survival promise.

Devenv containers are keyed by resolved image ID. The `devenv` image is
toolset-only and content-addressed in `platform.yml`, so a backend-only
release re-tags an identical image and the node adopts the running container;
a toolset change launches a sibling for new sessions while existing
containers keep serving their shells. The node delivers its per-release
`sulion-devenv` binary and the `sulion` CLI through the run volume. Sessions
record their host in `pty_sessions.devenv_ident`.

Moving a session to the current toolset is an explicit per-session action
that restarts the shell in place, keeping its identity and workspace binding.
Emptied non-current containers are reaped immediately. The node-to-devenv
socket protocol is additive from day one, for the same reason as ADR 0016.

## Alternatives considered

- **Keep masters in the node and improve orphan reconciliation.** The state
  before this change. The user rejected it ("all this reconcile orphans is
  junk right now"), and the plan's failure analysis shows why no
  reconciliation helps: a master fd dies with the process that holds it.
- **Treat each devenv container as a node, with pairing and TLS.** The first
  plan draft. The user stopped it ("why are there several nodes"). There is
  one node; devenv containers are containers the node launches on the same
  host and trust domain, reached over a local socket like `correlate.sock`.
- **Run the devenv as a compose service.** Rejected in the plan: the next
  node release's `--remove-orphans` would delete it. Devenv containers are
  directly launched, label-adopted containers, like the dev-postgres ones.
- **Devenv as a tag of the backend image.** Left open after the plan rewrite
  and resolved against (Decision 1): a separate toolset-only image is what
  stops the toolchain rebuilding, and therefore rolling containers, on every
  backend commit.
- **A thin per-commit binary layer on the devenv image.** Rejected
  (Decision 4): it mints a devenv image per commit and rolls containers on
  every release, defeating the toolset-only requirement in spirit. The node
  delivers the binary over the run volume instead.
- **A dual hosting period with the in-process path kept alive.** Rejected in
  the phase 1 expansion. Keeping the old path meant the e2e suite and the
  standalone role would never exercise the devenv protocol; one path means
  they exercise it continuously.
- **Upgrade by resuming the transcript in a fresh session.** Decision 2 chose
  restart in place with no agent-specific logic; resuming an agent remains
  the existing resume flow's job.
- **Retention window before reaping empty containers.** Decision 3: new
  sessions and upgrades only land on the current devenv, so an emptied
  non-current container can never refill and a window would preserve
  nothing.
- **Combined-role survival through a runner-launched devenv.** Noted as
  possible through the same launch-mode seam and deferred until asked for;
  the plan changes no PTY lifetime outside the dedicated node.

## Consequences

- Recreating the node container leaves shells running and they reattach
  with no gap in the shadow emulator. Host reboot and combined-role
  replacement still end their shells.
- Invariant 4 (the shadow emulator is fed continuously) now lives in the
  devenv server process, next to the masters.
- The `sulion_run` volume and the devenv socket path are part of the deploy
  contract; the deploy script never runs `compose down`.
- A stale CLI inside a long-lived shell is a real failure mode; the node
  re-links `/run/sulion/bin/sulion` on every start, including adoption.
- The node's legacy `node_id IS NULL` orphan sweep is gone; the persisted
  `orphaned` state remains readable for old rows.
- Shared toolset assets duplicated between `devenv/` and `backend/` are
  pinned byte-identical by `make validate-deploy`.

## References

- Current contract:
  [`../architecture.md`](../architecture.md#pty-lifetime-and-toolset-upgrades)
  and invariant 7; [`../deploy.md`](../deploy.md#pty-and-toolset-releases)
- The July 2026 PTY-survival plan and its three phase records are folded
  into this record; the release-surface analysis and decisions 1–5 above
  are theirs
- Decision amended: [ADR 0002](0002-hybrid-control-plane-and-dev-node.md)
- Commits: `8104eb8` (devenv server owns masters), `bcc6e82` (containers
  keyed by image identity), `6cd80c3` (per-session upgrade and reaping)
- CHANGELOG v2.1.0, "Session lifetime and toolsets"
