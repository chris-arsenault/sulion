# Phase 3 — Upgrade, drain, and cleanup: execution steps

Expansion of Phase 3 in [pty-survives-deploy.md](pty-survives-deploy.md).

Decisions 2 and 3 resolve under the standing least-brittle rubric:

- **Decision 2 — upgrade semantics: restart in place.** The session keeps its
  identity (same id, row, label, workspace binding); the shell process ends
  on the old devenv and a fresh default shell starts on the current one, in
  the same working directory. No agent-specific logic: resuming an agent is
  what the existing resume flow is for, and the plan's own out-of-scope note
  already said "Phase 3 restarts within the session".
- **Decision 3 — reap immediately.** New sessions only ever land on the
  current devenv and upgrades only move sessions *to* it, so a non-current
  container that reaches zero hosted sessions can never gain one again — a
  retention window would preserve nothing. A stopped non-current container's
  shells are already gone and is reaped regardless.

## Steps

1. `SessionUpgrade` request kind
   - File(s): `backend/src/node_protocol/model.rs`,
     `backend/src/node_runtime/requests.rs`, `backend/src/node_protocol/commands.rs`
   - Reference behavior: `NodeRequestKind` round-trips through `as_str`/`parse`
     strings; an unknown kind on an old node parses to `None` and is refused
     cleanly, so the addition is deploy-order-safe.
   - Change: add the variant + string; route it through the session group to
     `NodeRuntime::upgrade_session`.
   - Verify: greenfield-red — the kind does not exist; round-trip unit
     coverage comes with the existing kind tests if any, else via compile.

2. Manager: restart a session on the current devenv  [depends on #1]
   - File(s): `backend/src/pty/mod.rs`, `backend/src/node_runtime/mod.rs`
   - Reference behavior: `delete`'s kill-and-wait pattern (bounded wait on the
     link's fan-out); `spawn`'s `ON CONFLICT … WHERE state <> 'live'` row
     reuse — the same-id respawn only lands after the exit event has marked
     the row dead, so the wait must cover both the handle and the row.
   - Change: `PtyManager::upgrade(id, node_id, boot_id)`: refuse when the
     session already sits on the current ident; kill via the link; wait
     (bounded) for the handle to clear and the row to leave `live`; respawn
     the same id with the session's repo/working-dir/workspace and the
     default shell on the current devenv. `NodeRuntime::upgrade_session`
     wraps it with its node identity.
   - Verify: red — `pty_integration` test with two in-process devenvs: spawn
     A and B on "old", roll current to "new", upgrade A → A live on "new"
     (row `devenv_ident` updated, shell echoes), B untouched on "old". Fails
     to compile before the method exists.

3. Control route and UI action  [depends on #1]
   - File(s): `backend/src/api/routes.rs`, `backend/src/api/session_routes.rs`,
     frontend session context menu + API client
   - Reference behavior: `delete_session`'s forward-only path — the node must
     be connected; unlike delete there is no husk fallback, because an
     upgrade without a process is meaningless.
   - Change: `POST /api/sessions/:id/upgrade` forwarding `SessionUpgrade`;
     a session context-menu entry for live sessions. The node answers
     "already on the current toolset" when there is nothing to do; the UI
     surfaces it as an ordinary error toast.
   - Verify: red — route absent (404 in rest_integration if covered);
     UI wiring mirrors existing actions.

4. Reap emptied non-current containers
   - File(s): `backend/src/devenv/launcher.rs`, `backend/src/devenv/link.rs`
   - Reference behavior: the launcher's ensure loop already enumerates state
     every 60 s and owns the only Docker access; the link knows how many
     sessions each ident hosts (`session_host`).
   - Change: each pass lists `sulion.devenv`-labeled containers; any that is
     not the current image ID is removed when stopped, or when running with
     zero hosted sessions (`DevenvLink::hosted_sessions(ident)`). The
     current container and any container still hosting sessions are never
     touched.
   - Verify: red — pure decision function `should_reap(running, is_current,
     hosted)` unit tests fail before it exists; the docker plumbing follows
     the launcher's existing arg-builder test posture.

5. The in-process-era reconciliation comes out
   - File(s): `backend/src/pty/mod.rs`, `backend/src/main.rs`,
     `backend/tests/pty_integration.rs`
   - Reference behavior: `reconcile_orphans_on_startup` exists solely for
     rows whose processes died with a pre-devenv backend (`node_id IS NULL`);
     phases 1–2 stopped creating such rows. `PtyState::Orphaned` and its
     string stay — persisted rows still carry the value, and removing a
     persisted enum value without a rehydration guard is a known footgun.
   - Change: delete the sweep, its startup call, and its test.
   - Verify: red — the removed test names the removed fn (compile);
     `rg reconcile_orphans` finds nothing after.

6. Gates: `make ci`, `make test-rust-integration`, `make e2e`; docs — mark
   decisions 2/3 resolved in the main plan.
