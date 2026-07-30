# Phase 1 — Shells move into a devenv server: execution steps

Expansion of Phase 1 in [pty-survives-deploy.md](pty-survives-deploy.md). Read
that plan's "The four release surfaces" and "Shape" sections first; reference
behavior below cites them and the code directly.

Two as-built notes beyond the step list: the spawning connection's output pump
takes the PTY's from-birth broadcast subscription (a post-spawn subscribe
races the reader task and can drop the shell's opening bytes), and the
manager writes the `pty_sessions` row *before* asking the devenv to spawn (a
fast-exiting shell's `Exited` event must always find the row).

One deliberate refinement over the milestone text: there is **no dual hosting
period**. From the end of this phase every role hosts PTYs in the devenv server
process — container-direct where `SULION_DOCKER_MODE=direct`, child-process
otherwise. Keeping the old in-process path alive alongside the new one would
mean the e2e suite and standalone role never exercise the devenv protocol; a
single path means they exercise it continuously. Phase 3's removal work shrinks
to reconciliation bookkeeping accordingly.

Deploy-ordering rule (see the cleanup plan's record): control plane and node
release independently, so every wire and store change below must be safe in
either order. The step 9 store change is deliberately additive for this reason.

## Steps

1. Make the PTY environment policy serializable
   - File(s): `backend/src/pty/environment.rs`, `backend/src/pty/mod.rs`
   - Reference behavior: `configure_pty_environment` mutates a
     `portable_pty::CommandBuilder` with the PTY env policy (SULION_PTY_ID,
     docker mode, broker URL, PATH shims, …). `shell_command` in `pty/mod.rs`
     applies it plus TERM.
   - Change: refactor it to return `Vec<(String, String)>` (pure function of
     its inputs); `shell_command` applies the pairs. No policy change — the
     pairs must cross the devenv wire in later steps, and `CommandBuilder`
     does not serialize.
   - Verify: greenfield-red — a unit test asserting the returned pairs contain
     `SULION_PTY_ID` and the broker vars fails to compile before the signature
     exists. `cargo test -p sulion` green after; `pty_integration` unchanged
     before/after (characterization).

2. Extract the process-owning PTY core, free of Postgres
   - File(s): `backend/src/pty/host.rs` (new), `backend/src/pty/mod.rs`
   - Reference behavior: `PtyManager::spawn` lines 259–386 — clamp size,
     `openpty`, spawn shell, drop slave, reader/writer/resize/supervisor
     tasks, emulator fed unconditionally (architecture invariant 4), master
     kept alive in `Arc<Mutex<…>>`; `delete` — SIGTERM, 3 s grace, SIGKILL.
   - Change: `HostedPty::spawn(HostSpawnSpec) -> HostedPty` where
     `HostSpawnSpec` is `{id, shell, args, cwd, env pairs, cols, rows}` and
     `HostedPty` exposes the output broadcast, input/resize senders, emulator,
     pid, an exit watch, and a `kill()` with today's TERM→grace→KILL ladder.
     Move `clamp_pty_size`, the four task spawners, and `wait_for_exit` in.
     `PtyManager::spawn` delegates to it; every DB statement stays in
     `pty/mod.rs`.
   - Verify: greenfield-red on the new symbols. New unit test: spawn
     `/bin/bash -c 'echo marker'` via `HostedPty` alone (no pool), receive
     "marker" on the broadcast, exit watch fires. `pty_integration` stays
     green.

3. Devenv wire protocol  [depends on #1]
   - File(s): `backend/src/devenv/mod.rs` (new), `backend/src/devenv/protocol.rs`
     (new), `backend/src/lib.rs`
   - Reference behavior: `correlate.rs` — newline-delimited JSON on a unix
     socket, absent-tolerant serde (`serde(default)`, unknown fields
     ignored). The plan's version-skew rule: additive protocol, no
     version-ordering handshake.
   - Change: two tagged enums. Node→devenv: `Spawn{spec, reply_id}`,
     `Input{id, bytes b64}`, `Resize{id, rows, cols}`, `Kill{id}`,
     `SnapshotRequest{id, reply_id}`. Devenv→node: `Hello{pid, sessions:
     Vec<InventoryEntry>}`, `SpawnResult{reply_id, ok, error}`,
     `Output{id, bytes b64}`, `Snapshot{reply_id, bytes b64}`,
     `Exited{id, exit_code}`. Every non-essential field defaulted. Shared
     line-framing read/write helpers.
   - Verify: unit tests — a message with unknown extra fields parses; a
     message missing defaulted fields parses (red: module absent).

4. Devenv server: sessions that outlive the connection  [depends on #2, #3]
   - File(s): `backend/src/devenv/server.rs` (new)
   - Reference behavior: plan "Shape" — reconnecting is the devenv's job;
     inventory on every (re)connect; emulator fed with no clients attached
     (invariant 4). Ordering: messages on one connection are ordered, so a
     `Snapshot` written after preceding `Output` frames reflects exactly the
     bytes already sent — the attach flow in step 7 depends on this.
   - Change: `DevenvServer` holding `HashMap<Uuid, HostedPty>`; a
     `serve(stream)` loop that sends `Hello` with the current inventory,
     pumps every session's output broadcast into `Output` frames, dispatches
     inbound messages, and emits `Exited` from the exit watch. Sessions
     persist across `serve` calls; a dropped connection changes nothing about
     the process tree.
   - Verify: tokio test over an in-process stream pair: spawn a session,
     receive output; **drop the node end, reconnect, `Hello` lists the
     session, and a snapshot returns the pre-drop output** (red: type
     absent). This test is the survival property in miniature.

5. `sulion-devenv` binary, registered as a backend-image artifact  [depends on #4]
   - File(s): `backend/src/bin/sulion_devenv.rs` (new), `platform.yml`,
     `backend/Dockerfile`
   - Reference behavior: `bin/sulion_node.rs` env-driven config style;
     `platform.yml` `rust_artifacts.binaries` drives which dist binaries CI
     copies into which image; Dockerfile `COPY dist/…` + chmod block. Phase 1
     ships in the backend image per the plan — no new image yet.
   - Change: main() dials `SULION_DEVENV_SOCK` (default
     `/run/sulion/devenv.sock`) and runs `DevenvServer::serve` per
     connection; redial with backoff. `SULION_DEVENV_EXIT_ON_DISCONNECT=1`
     makes it exit when the connection closes (child mode). Add
     `{bin: sulion-devenv, image: backend}` and the COPY/chmod lines.
   - Verify: red — `cargo build --bin sulion-devenv` fails before the file
     exists; green after. `docker compose config` render targets in CI stay
     green.

6. Node-side devenv link  [depends on #3]
   - File(s): `backend/src/devenv/link.rs` (new)
   - Reference behavior: `correlate::run` — stale-socket removal, bind,
     accept loop on the shared volume. `node_runtime::open_terminal` needs,
     per session: an output `broadcast::Sender`, input/resize/kill, and a
     snapshot. RPC replies correlate on `reply_id`.
   - Change: `DevenvLink` — `UnixListener` on `SULION_DEVENV_SOCK`; on
     `Hello`, invoke an adoption callback with the inventory and (re)build
     per-session handles; route `Output` into each handle's broadcast,
     `Exited` into an exit callback; expose
     `spawn(spec)` / `input` / `resize` / `kill` / `snapshot(id)` with
     await-able replies and a clear error when no devenv is connected.
   - Verify: integration test wiring `DevenvLink` to `DevenvServer` over a
     tmp socket path: spawn through the link, output arrives via the handle's
     broadcast, snapshot RPC round-trips (red: type absent).

7. PtyManager keeps the records, the link does the processes  [depends on #6]
   - File(s): `backend/src/pty/mod.rs`, `backend/src/node_runtime/mod.rs`,
     `backend/src/node_runtime/requests.rs`
   - Reference behavior: today's DB semantics, unchanged: spawn inserts the
     `pty_sessions` row (`ON CONFLICT … WHERE state <> 'live'`), `mark_dead`
     cascades agent_runtime fields, `delete` marks deleted and revokes the
     broker credential; `secret_pty::prepare_pty_credential` runs before
     spawn and its key path (under `/run/sulion/pty-keys`, on the shared
     volume, visible to the devenv) rides the env. `open_terminal`: snapshot,
     `terminal.ready`, then live output; the step 4 ordering guarantee
     replaces the in-process snapshot-then-subscribe.
   - Change: `PtyManager` takes the `DevenvLink`; `spawn` = prepare
     credential → env pairs (step 1) → `link.spawn` → DB insert → register
     handle; `send_input`/`delete` go through the link; link exit events
     drive `mark_dead`; `PtySession` becomes the link handle (output
     broadcast + async `snapshot()`), and `open_terminal` awaits it. The
     in-process spawn path in `pty/mod.rs` is deleted — `HostedPty` is now
     reachable only through the devenv server.
   - Verify: red — `pty_integration` and `ws_integration` fail to compile
     against the new shapes; green after rewiring with an in-process
     `DevenvServer`↔`DevenvLink` harness in `backend/tests/common`. Add one
     targeted test: spawn → live row + echoed input; kill → row dead with
     exit code.

8. Devenv launcher: child mode and container mode  [depends on #5, #7]
   - File(s): `backend/src/devenv/launcher.rs` (new),
     `backend/src/bin/sulion_node.rs`, `compose.yaml`,
     `deploy/compose.dedicated.yaml`
   - Reference behavior: `container_runner/postgres.rs` — reuse by
     `ps --filter label=`, validate labels before adopting, explicit
     `docker run` arg construction. The dedicated node runs host-network with
     `/home/sulion` and the `sulion_run` volume (`deploy/compose.dedicated.yaml`);
     PTY dev servers must bind LAN ports 26000–26010, so the devenv container
     is host-network too. Phase 1 image = the node's own backend image.
   - Change: at node startup, `SULION_DOCKER_MODE=direct` → ensure a
     label-owned (`sulion.devenv`) container named `sulion-devenv` exists and
     runs `sulion-devenv` from `SULION_DEVENV_IMAGE` (compose passes the
     backend image ref) with host network, `/home/sulion`, and the run
     volume; adopt it if already running — never recreate. Any other mode →
     spawn the `sulion-devenv` child process (exit-on-disconnect), respawning
     it if it dies. Wire the env into both compose files.
   - Verify: unit test on the generated `docker run` args and label
     validation (no daemon needed, matching the postgres.rs test approach).
     Child mode is covered end-to-end by step 10's suites. Red: launcher
     module absent.

9. Boot adoption: a node restart stops ending devenv sessions  [depends on #7]
   - File(s): `backend/src/node_protocol/store.rs`,
     `backend/src/bin/sulion_node.rs` (hello inventory timing)
   - Reference behavior: `store.rs` `end_sessions_from_prior_boot` kills every
     live row from a previous boot on hello; `reconcile_missing_sessions`
     kills current-boot rows absent from the inventory. The node's hello
     inventory is `live_session_ids()`, which after step 7 reflects the
     devenv link. Deploy-ordering: this is a control-plane behavior change
     and must be additive — an old control plane facing a new node merely
     keeps today's (lossy) behavior; a new control plane facing an old node
     sees no prior-boot live inventory and changes nothing.
   - Change: in the hello transaction, before `end_sessions_from_prior_boot`,
     re-stamp rows named in the live inventory to the current boot id
     (`node_id` must match). Node side: wait briefly for the devenv `Hello`
     before the first control connect so the inventory is honest; on timeout
     connect anyway with what is known.
   - **As built (refinement):** the hello is *signed* over an explicit field
     list and carries no inventory — inventory already flows through
     heartbeats. So instead of extending the hello, `record_connection`
     stopped ending prior-boot sessions entirely, and the heartbeat now (a)
     adopts listed live rows onto the current boot id and (b) ends any live
     row for the node — whatever its boot — missing from a complete
     inventory. Zero wire changes; both deploy orders degrade to today's
     behavior at worst. The node-side wait landed as specified.
   - Verify: `node_protocol_integration` new test, red today: a live row from
     boot A, hello for boot B listing it live → row stays live stamped with
     boot B (currently it dies with `runtime_end_reason = 'node_reboot'`).

10. e2e and CI wiring  [depends on #8]
    - File(s): `scripts/run-e2e-stack.mjs`,
      `scripts/run-backend-integration-tests.sh`, `Makefile` (only if a new
      test target file is added)
    - Reference behavior: the e2e stack copies `target/debug/sulion-node`
      into the image dist dir and runs the node with
      `SULION_DOCKER_MODE=none` — after step 8 that means child mode, which
      needs the `sulion-devenv` binary present in the image. Working rule:
      new integration targets are registered in the runner script, never
      `#[ignore]`d.
    - Change: copy `target/debug/sulion-devenv` into the e2e dist dir; if
      devenv tests landed in a new `backend/tests/devenv_integration.rs`,
      add it to `TEST_TARGETS`.
    - Verify: red — the e2e stack fails at node PTY spawn without the binary
      in the image; `make ci`, `make test-rust-integration`, and the
      Playwright suite green after.

11. Doc sync
    - File(s): `docs/architecture.md`, `docs/deploy.md`
    - Reference behavior: architecture invariants name the node as PTY owner;
      deploy.md states "Replacing `sulion-node` is an explicit
      session-affecting operation" — both stop being true at this phase's
      exit gate.
    - Change: current-state assertions only — PTY masters live in the devenv
      server; a node replacement leaves shells running; invariant 2/4
      wording follows the emulator to the devenv process.
    - Verify: no automated red→green (docs); `make ci` doc checks stay green.
      Reviewed against the exit gate below.

## Exit gate (from the milestone)

Recreating the node container (surface 3 semantics: `compose up` recreation of
`sulion-node` only) leaves shells running; they reattach with no gap in the
shadow emulator; the `node_id IS NULL` orphan sweep stops firing; the e2e stack
passes with its Docker-less node. Step 4's reconnect test and step 9's adoption
test are the in-tree approximations; the real gate is verified on
`sulion-enclave` after a release, per the deploy-verification rule (a session
recycle is not a healthy deploy).

**Gate status as executed:** all three gates are green — `make ci`,
`make test-rust-integration` (all 11 targets, including the survival and
adoption tests), and `make e2e` (13 passed, 1 conditionally skipped). The e2e
clause had no baseline to preserve: the suite had been unrunnable since the
Jul 27–28 node-security cutover left the harness speaking retired contracts
(`USER dev` in Dockerfile.e2e, `/tmp` bind mounts invisible to the host
daemon, the removed enrollment-token flow, no pairing approval or
delivered-config activation, missing `HOME`, a shared broker/backend
database, an auth-gated broker `/health`). Restoring it was worth the trouble
— once running, the suite caught two real bugs in the devenv PTY path, both
fixed here:

- **Keystroke transposition**: node commands are handled concurrently, so
  two terminal-input frames could enqueue onto the devenv link in either
  order. Terminal commands are now handled inline in envelope order on the
  node (`client.rs`), and the link enqueues under its state lock.
- **Truncated final output**: `child.wait()` returns at process death while
  the shell's last bytes are still in the kernel PTY buffer, and the exit
  frame tore down the node-side fan-out ahead of them. The reader now
  signals EOF and the pump flushes output (bounded) before `Exited`.

It also surfaced a real frontend bug: the vite dev server lacked the
`global` shim `amazon-cognito-identity-js` needs, hard-crashing the app on
load under `pnpm dev`.
