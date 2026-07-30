# Phase 2 — Versioned tags: execution steps

Expansion of Phase 2 in [pty-survives-deploy.md](pty-survives-deploy.md).
Reference sections: "Shape" (binary delivery through the run volume, decision
4), "The four release surfaces" (surface 1 mechanics), and decisions 1 and 5
(toolset-only image, content-addressed builds, node rolls on image identity).

The container identity used throughout is the Docker **image ID** of the
current `SULION_DEVENV_IMAGE` reference — per decision 5 it changes exactly
when toolset content changes, while the `:sha` tag changes every release.
Child-mode devenvs (no Docker) have no image identity and report none.

## Steps

1. Protocol: a devenv announces its identity
   - File(s): `backend/src/devenv/protocol.rs`, `backend/src/devenv/server.rs`,
     `backend/src/bin/sulion_devenv.rs`
   - Reference behavior: `Hello` is additive (absent-tolerant fields, no
     version handshake). Phase 1 devenvs and child mode send no identity and
     must keep working.
   - Change: `Hello` gains `#[serde(default)] ident: Option<String>`.
     `DevenvServer` carries an optional ident (from `SULION_DEVENV_IDENT` in
     the binary) and includes it in every hello.
   - Verify: greenfield-red — protocol tests naming `ident` fail to compile;
     a hello without the field still decodes (additive test).

2. Link: connections keyed by identity, ops routed to the session's host
   [depends on #1]
   - File(s): `backend/src/devenv/link.rs`
   - Reference behavior: the milestone — one devenv container per tag, all
     connected at once; existing containers keep serving their shells. Today
     the link holds a single `outbound` and a newer connection supersedes.
   - Change: `LinkState` keeps `connections: HashMap<String, Sender>` (key:
     ident, `"default"` when absent) and `session_host: HashMap<Uuid, String>`
     filled from each hello's inventory and each spawn. `spawn` targets an
     explicit ident (the current one); input/resize/kill/snapshot route via
     `session_host`. `LinkEvent::Connected` carries the ident. A connection
     drop clears only its own entry.
   - Verify: red — new test wiring two in-process `DevenvServer`s with
     distinct idents to one link: spawn to each, input routes to the right
     shell, snapshot round-trips per host, one connection dropping leaves the
     other serving.

3. Sessions record their hosting identity  [depends on #2]
   - File(s): `backend/migrations/` (new `ALTER TABLE pty_sessions ADD COLUMN
     devenv_ident TEXT`), `backend/src/pty/mod.rs`
   - Reference behavior: the milestone — "records on each session which tag
     it is on". Additive nullable column (rehydration-safe; no enum).
   - Change: spawn writes the target ident on the insert; adoption re-stamps
     `devenv_ident` from the announcing connection's ident.
   - Verify: red — `pty_integration` test asserting the row's `devenv_ident`
     matches the hosting server's ident fails (column absent) before the
     migration.

4. Launcher: one container per image identity, binary from the run volume
   [depends on #1]
   - File(s): `backend/src/devenv/launcher.rs`
   - Reference behavior: decision 4 (node writes a per-release copy of
     `sulion-devenv` onto the shared run volume; a per-release path is never
     overwritten) and decision 5 (roll only when the image ID differs).
     Adoption posture from `container_runner/postgres.rs`: validate labels,
     never recreate a running container.
   - Change: container mode resolves the current image ID (pull if absent),
     names the container `sulion-devenv-<id12>`, labels it with the image ID,
     adopts an existing match, and launches one when missing — leaving every
     other devenv container untouched. Before launching, copy the node's own
     `sulion-devenv` binary to `/run/sulion/devenv-bin/<hash16>/sulion-devenv`
     (content-hash path, write-once) and exec that path through the image
     entrypoint with `SULION_DEVENV_IDENT=<image-id>`.
   - Verify: red — unit tests on the new arg builder (name, labels, ident
     env, volume exec path) and on the binary-delivery path layout fail
     before the change.

5. The devenv image joins the build  [depends on #4]
   - File(s): `devenv/Dockerfile` (new, toolset-only), `devenv/` copies of the
     PTY-facing assets (`bin/` wrappers, `hooks/`, `entrypoint.sh`,
     `docs/toolset.md`), `platform.yml`, `deploy/compose.dedicated.yaml`,
     `Makefile`
   - Reference behavior: decision 1 — toolset-only image, no per-commit Rust
     binaries; the backend image keeps its own workbench for the combined
     role (plan: out of scope to slim it). CI builds each component from its
     own directory, so shared assets are duplicated into `devenv/` and kept
     identical by a check rather than by convention.
   - Change: `devenv/Dockerfile` = the backend Dockerfile's toolset layers
     plus the PTY assets, ending at the entrypoint (no `dist/` binaries —
     the launcher delivers the server binary). `platform.yml`: `images` +=
     `devenv`, `content_addressed_images: [devenv]`. Compose:
     `SULION_DEVENV_IMAGE` points at `devenv:${IMAGE_TAG}`. `validate-deploy`
     gains a diff of the duplicated assets so backend/ and devenv/ cannot
     drift silently.
   - Verify: red — the new drift check fails before the copies exist;
     `make validate-deploy` (compose render + asset diff) green after.
     The image itself builds on surface 1; not built locally by default.

6. Gates
   - `make ci`, `make test-rust-integration`, `make e2e` — the e2e stack runs
     child mode and must be untouched by all of this. The "rolling the tag"
     exit gate is exercised in-tree by step 2's two-host test and step 4's
     launcher tests; the live gate is verified on the enclave after a
     release, per the deploy-verification rule.

## As built

- The wrappers in `/opt/sulion/bin` exec the per-commit `sulion` CLI, so the
  toolset image cannot bake it either: the node delivers **two** binaries
  through the run volume — `sulion-devenv`, exec'd at its versioned path at
  container launch, and `sulion`, resolved through a stable
  `/run/sulion/bin/sulion` symlink the node swaps atomically per release
  (the image's `/usr/local/bin/sulion` is a symlink into the volume,
  dangling until mounted). The CLI's node-facing protocols are additive, so
  running shells tolerate the version moving forward — the same contract a
  node deploy imposed on every shell before this plan.
- The current ident lives on the link (`set_current_ident`), set by the
  launcher after resolving the image ID; child mode leaves it at
  `"default"`. `LinkEvent::Connected` carries the ident, and dead-marking on
  a hello is scoped to sessions that devenv hosts.
- Session rows record the host in `devenv_ident` (migration 0067), written
  after spawn (the spawn decides the host) and re-stamped on adoption.

## Out of scope for this phase

- Reaping emptied containers and the per-session upgrade action (Phase 3;
  decisions 2 and 3 remain open).
- Slimming the backend image.
