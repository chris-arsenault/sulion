# Cleanup and hardening plan

Derived from a full review of the repo (legacy cruft, dead code, architectural
cleanliness, security implementation bugs). Items are grouped into chunks that
can be taken one at a time; within a chunk, items are independent unless a
dependency is stated.

**Deploy-ordering constraint applies throughout.** The control plane is deployed
by Komodo; the dedicated NixOS node polls for releases on its own timer. They run
the same image but are *independently released with an arbitrary gap*. Any item
touching a contract between them must work in either deploy order — including
contracts that have nothing to do with the node protocol, such as the correlate
socket payload (item 28), the HTTP request shape a stale browser tab still sends
(item 29), and persisted columns an older instance may still write (item 30).

---

## Chunk 1 — Protocol compatibility (closed — no work outstanding)

Kept as a record so this is not re-raised by a future review. The original
proposal here was to replace exact-version matching with range negotiation. That
was premature and is not being pursued: the v2 bump it was written against has
been reverted (`NODE_PROTOCOL_VERSION` is back to `1`), and the compatibility
problem it was meant to solve has been answered by policy instead of machinery.

1. **Range negotiation — not pursued.** `docs/node-protocol.md` now states the
   rule directly: protocol changes are additive, new payload fields are optional,
   a reader treats an absent field as "not reported", no message uses
   `deny_unknown_fields`, and *adding a field is never a reason to bump
   `NODE_PROTOCOL_VERSION`*. A bump is reserved for a message that genuinely
   cannot be served by both builds, and the doc requires such a change to carry
   its own compatibility plan. Under that rule the exact-version checks at
   `node_protocol/mod.rs:312,374,470,732` never fire, so negotiation would be
   speculative machinery. Build it only if a bump ever becomes genuinely
   unavoidable — and then as part of that change's compatibility plan.

2. **Absent-tolerant fields — keep, permanently.** `#[serde(default)] node_nonce`
   (`node_protocol/model.rs:37-38`) and the `inventory_complete` default, now
   alongside `host: Option<NodeHostStats>`, all three at
   `node_protocol/mod.rs:172-179`, with the inventory conditional at
   `node_protocol/store.rs:164`. An earlier pass flagged the first two as cruft on
   the assumption that strict version equality had already severed old peers.
   That assumption was wrong. These are the mechanism that makes either deploy
   order work; new payload fields go in the same shape.

3. **Nonce-appending in `NodeHello::signing_payload`** (`model.rs:56-61`) — leave
   as is. With no bump pending there is nothing for it to ride, and changing the
   signed construction would sever peers for no benefit.

4. **Heartbeat documentation — done.** `docs/node-protocol.md` now describes the
   heartbeat as boot identity, live PTY inventory, and a whole-machine resource
   sample, and records that control keeps the latest sample per connected node,
   drops it on disconnect, and neither series-collects nor persists it.

---

## Chunk 2 — Security implementation bugs (done)

All five applied. What landed, in case a later reader needs the shape rather
than the diff:

- Item 5 became a module extraction: the render dispatch moved to
  `frontend/src/components/fileRenderKind.ts` so the decision that governs
  whether file bytes reach the DOM as markup is directly testable, and SVG now
  resolves to the existing blob-`<img>` path. `SvgBody`, the `image-svg` render
  kind, and the `.ft__svg` rule are gone, so there is no longer a code path from
  a repo file to `dangerouslySetInnerHTML`. The raw toggle now works for SVG
  (it was suppressed before), so the source is still inspectable as inert text.
- Item 7 was fixed at the two chokepoints inside `pty/mod.rs` rather than at the
  call sites, because both the node and local resize paths converge on the
  resize channel. The duplicate clamp in `node_runtime/mod.rs` was removed so
  the bounds exist once.
- Item 8 caps the per-tick read *and* handles the line that outgrows it, which
  the original item did not specify: an over-long line is skipped forward to the
  next boundary via `next_line_boundary`, so the ingester resynchronises instead
  of stalling on it forever.
- Item 9 was resolved by softening the doc, not changing the design.

Verification: backend `cargo check`/`clippy --all-targets` clean and 211 unit
tests pass (5 new); frontend `tsc --noEmit` and `eslint` clean and 259 tests
pass (4 new).

The backend integration suite could not run when this chunk landed, so the
ingest change (item 8) shipped without coverage of `process_file`. The harness
has since been fixed (see Chunk 4) and the suite passes, including
`ingester_integration`, so that gap is closed.



5. **Sanitize inlined SVG.** `backend/src/api/file_content.rs:79-84` inlines
   `image/svg+xml` as text; `frontend/src/components/FileTab.tsx:381-383` passes
   it to `dangerouslySetInnerHTML`. `innerHTML` blocks `<script>` but not SVG
   event handlers (`<animate onbegin=>`, `<foreignObject><img onerror=>`). An
   agent writing `docs/diagram.svg` gets JS execution in the app origin when the
   file is opened, with Cognito tokens in `localStorage`.
   *Fix:* DOMPurify with the SVG profile, or route SVG through the existing
   `AuthenticatedImage` blob-URL path (an SVG inside `<img>` cannot script).
   *Verification:* a test fixture SVG with an `onbegin` handler renders inert.

6. **Fix the byte-slice panic on non-ASCII prompts.**
   `backend/src/ingest/timeline/project.rs:617` does
   `&first[..max.saturating_sub(1)]` on a `String`. The sibling helper at
   `ingest/timeline/render.rs:292` already uses `chars().take(keep)`; mirror it.
   Reachable from the ingester on every insert, from `POST /api/admin/reindex`,
   and from startup maintenance. Impact: that session's projection never
   completes and the ingester restarts every second on each subsequent line.
   *Verification:* unit test with ~140 Cyrillic characters and with an emoji
   landing on the boundary.

7. **Clamp terminal resize.** Spawn clamps at `node_runtime/mod.rs:347-348`
   (`cols.clamp(20,500)`, `rows.clamp(5,300)`); the resize path has no clamp
   anywhere — `api/ws.rs:282` (node path) and `:393-402` (local path) pass
   straight through to `vt100::Parser::set_size`, which eagerly materializes both
   the normal and alternate grid. `{"cols":65535,"rows":65535}` is ~275 GB of
   eager allocation, i.e. `handle_alloc_error` and an **abort** — not a catchable
   panic — killing every PTY on the box. Apply the spawn bounds in the
   `SessionResize` handler (`node_runtime/requests.rs:111-128`) and the local
   `ws.rs` path.
   *Verification:* test that an out-of-range resize is clamped, not propagated.

8. **Bound the transcript delta read.** `ingest/ingester.rs:337-340` buffers the
   entire appended region with `read_to_end`, and a line with no trailing newline
   is never committed, so it is re-read in full every 500 ms tick with no cap. A
   multi-GB unterminated line OOM-loops ingestion for the whole node. Cap the
   per-tick read and skip a line that exceeds a sane maximum, preserving
   invariant 3 (tolerate partial lines).

9. **Settle the `docs/secrets.md` mismatch.** The doc states grants are scoped
   per-PTY and the wrapper signs with the PTY private key, but every PTY runs as
   uid 7321 and the keys under `/run/sulion/pty-keys/` are mutually readable, so
   the scoping is not enforced by anything. This is a property of the vetted
   single-uid design, not a defect — but the doc currently claims a guarantee the
   system does not provide. Either soften the doc or change the design; this item
   is only "make the written contract match reality."

---

## Chunk 3 — Dead code (done)

All twelve applied; 846 lines removed. Notes worth keeping:

- Item 14 (`terminal_attach_channel`) was flagged "confirm first" because
  `node_runtime/mod.rs` was mid-edit. With host stats committed it is confirmed
  unreferenced — a leftover, not scaffolding.
- Items 15 (`send_blocking`) and 16 (the two `EmbeddingResponse` fields) were
  marked "your call" and optional. Both were removed. `send_blocking` carried a
  comment claiming diagnostic value but had no caller anywhere, including the
  hook script it named.
- Deleting the exports surfaced four more dead symbols the review had not
  found, because they were only reachable from the code being removed:
  `clearRepoExpansionStorage` and a `clearLastViewedStorage` import in
  `SessionStore.tsx`, a `WorkspaceView` import in `api/client.ts`, and the
  `anyhow!` macro import in both `code_intel/indexer.rs` and `parser.rs`.
- The frontend uses **pnpm**, not npm. `pnpm install --lockfile-only` updates
  `pnpm-lock.yaml`; `npm install` crashes on the pnpm-shaped `node_modules`.

Verification: `cargo clippy --all-targets` clean, 211 backend unit tests pass;
`tsc --noEmit` and `eslint .` clean, 253 frontend tests pass (down from 259 —
the 6 removed belonged to the dead `timeline/types.ts`).



### Backend (~105-115 lines)

10. `resolve_current_root` — `backend/src/code_intel/indexer.rs:533-586`, plus the
    three private helpers used only by it: `basename` (`:609`), `env_path`
    (`:616`), `env_optional` (`:620`). Role is served by the live
    `discover_allowed_root_specs` / `index_pending_allowed_roots`. ~70 lines.
11. `wrapper_path` + `DEFAULT_WRAPPER_PATH` — `backend/src/codex.rs:25-32`, `:13`.
12. `ParsedSource::root_range` — `backend/src/code_intel/parser.rs:195-201`; it is
    the sole member of its `impl`, so the whole block goes. The struct is live.
13. `language_required` — `backend/src/code_intel/parser.rs:364-367`.
14. `terminal_attach_channel` + `TERMINAL_ATTACH_BUFFER` —
    `backend/src/node_runtime/mod.rs:566-568`, `:25`. **Confirm first**: this file
    is mid-edit for the host-stats work; it may be scaffolding rather than a
    leftover.
15. `send_blocking` — `backend/src/correlate.rs:492`, already under
    `#[allow(dead_code)]` and documented as kept for diagnostics. Live path is
    `send_blocking_for_agent` (`:568`). **Your call** whether the doc comment
    means keep.
16. Optional: `EmbeddingResponse.model` / `.usage` —
    `backend/src/retrieval/embeddings/client.rs:117-120`, deserialized and never
    read. Serde does not require them; some prefer keeping them as wire-format
    documentation.

### Frontend (~720 lines)

17. **Four unused UI primitives, ~440 lines.** `components/ui/Lane.tsx` (51) +
    `Lane.css` (108), `Panel.tsx` (38) + `Panel.css` (43), `Stat.tsx` (34) +
    `Stat.css` (47), `Tab.tsx` (40) + `Tab.css` (65). Exported only through the
    barrel `components/ui/index.ts` (lines 10, 14-16 and the CSS imports at 1,
    3-5); nothing renders them, and the CSS ships in the bundle solely via those
    barrel imports. The `LaneSize` type dies with them.
18. **`components/timeline/types.ts` + `types.test.ts`, 237 lines.** All 15
    exported helpers and the `ToolResultBlock`/`ThinkingBlock` types are
    referenced only in-file and by the test; the timeline components use
    `api/types.ts`. The test only exercises dead code.
19. Unused API client wrappers — `listWorkspaces` (`api/client.ts:552`),
    `getWorkspace` (`:556`), `getWorkspaceFiles` (`:587`). Workspace data arrives
    via `/api/app-state`. Backend routes stay; these are unused wrappers.
20. Strays — `resetSessionStoreStorage` (`state/SessionStore.tsx:367`),
    `thinkingBlock` (`components/timeline/test-helpers.ts:60`),
    `expectTerminalNotToContain` (`e2e/helpers.ts:185`).
21. Redundant devDeps — `@typescript-eslint/eslint-plugin` and
    `@typescript-eslint/parser`; both are bundled by the `typescript-eslint`
    meta-package that `eslint.config.js:2` actually imports.

**Do not remove:** `LEGACY_SECRET_GRANT_TOOLS` (`api/client.ts:104`, used at
`:504`/`:516`) posts each grant once per tool name for broker back-compat with
`dedupeSecretGrants` collapsing duplicates on read. Real cruft, but the broker
must drop its per-tool grant model first — see item 34.

---

## Chunk 4 — Drifting duplication (done)

All three applied, plus a fix to the test harness that was blocking
verification.

- Item 22 was a real bug, not a style gap: `sulion-code` built request URLs
  without the trailing-slash base, so a gateway-prefixed
  `SULION_CODE_INTEL_URL` silently lost its prefix, and it used an unpinned
  `reqwest::Client`. Both now match `sulion-retrieve`.
- Item 23 extracted `backend/src/cli_http.rs`. `build_url` is the shared
  version, so the bug in item 22 cannot recur in one CLI only.
- Item 24 extracted `backend/src/node_protocol/commands.rs`: one
  `handle_command` over a `CommandSink` trait, implemented by the websocket
  client and the loopback. This closes the coverage asymmetry the review
  flagged — integration tests exercise the loopback, which is now the same code
  the real node runs. `control.node_config` stays client-only, since loopback
  shares the process it would configure.
- The `unsupported_request` message differed between the two copies. Unified as
  two explicit cases: an unparseable kind reports "not supported by this node
  release", a runtime-less connection reports "not supported by this node".
- **Harness fix.** `scripts/run-backend-integration-tests.sh` chose the database
  address from which `docker` binary was on `PATH`, assuming a brokered runner
  meant the caller shared a network with the container and could address it by
  name. In a managed PTY that is false, and the suite died at `db::connect`. It
  now always publishes the port and *probes* which address works, preferring the
  published one. The full suite runs here: 11 targets, 110 tests, green.

Verification: `cargo clippy --all-targets` clean, 216 backend unit tests pass
(5 new in `cli_http`), and the full integration suite passes — notably
`node_protocol_integration` (16) over the unified dispatch, `ingester_integration`
(20), and `pty_integration` (6).

---

## Chunk 4 — original items

22. **Port the two missing fixes to `code_cli.rs` now.** The uncommitted
    trailing-slash fix at `retrieval_cli.rs:131-134` (so `Url::join` preserves a
    path-prefixed base URL, with a regression test) is absent from
    `code_cli.rs:400-403` — `SULION_CODE_INTEL_URL` with a path prefix silently
    drops the prefix today. Separately, `code_cli.rs:53` uses a bare
    `reqwest::Client::new()` where `retrieval_cli.rs:34` uses the pinned-cert
    `control_http_client()`.
23. **Then hoist the shared CLI code into a `cli_http` module.** ~150 duplicated
    lines: byte-identical `env_required`/`env_optional`/`infer_repo`
    (`retrieval_cli.rs:552-580` vs `code_cli.rs:507-535`), plus `insert_header`,
    `request_json`, and the `CliEnv` shape. `plan_cli.rs`/`plans.rs` is the model
    to copy: one implementation, thin callers.
24. **Unify node-protocol command dispatch.** `node_protocol/client.rs:451-547`
    and `loopback.rs:151-250` hand-roll the same `request` / `terminal.attach` /
    `detach` / `input` / `resize` handlers with no shared trait (~110 lines,
    including identical `EphemeralRequest` structs at `client.rs:74-78` and
    `loopback.rs:14-18`, plus near-identical `ensure_request_succeeded` and
    heartbeat loops). Host-stats had to be patched into both independently
    (`client.rs:403`, `loopback.rs:76,111`). Already diverged: different error
    text (`client.rs:650` vs `loopback.rs:176`) and parallel-vs-serialized
    execution (`client.rs:430-436` spawns; `loopback.rs:121-127` awaits inline).
    Integration tests cover only loopback (`tests/node_protocol_integration.rs:452`),
    so the real client's copy is the untested one.
    *Fix:* one shared `handle_command` over a small sink trait.

---

## Chunk 5 — Single-component legacy cruft (done)

Items 25 and 26 applied. Item 27 was **not** done here: it says to fold into
item 31, and Chunk 7 restructures the same health-check block, so fixing it
separately would only create a conflict.

- Item 25: the three `SULION_DB_URL.or_else(DATABASE_URL)` fallbacks are gone
  and the error text now names only `SULION_DB_URL`. Checked first that no
  `sqlx::query!`-style macro exists, since those read `DATABASE_URL` at compile
  time — all queries here are runtime string queries, so nothing needs it for a
  build. `container_runner/postgres.rs` still injects `DATABASE_URL` and
  `TEST_DATABASE_URL` into user dev containers, which is the documented feature
  (`docs/development.md:54`), untouched.
- Item 26: the `"worktree"` workspace-*mode* alias is gone from all five sites.
  Workspace *kind* `"worktree"` is a distinct persisted value — used across
  `worktree.rs`, the sidebar, and `workspace_integration` — and is untouched.

Verification: `cargo clippy --all-targets` clean, 216 unit tests pass, and the
full integration suite passes (11 targets, 110 tests), including
`workspace_integration` and `rest_integration`, which exercise both workspace
modes end to end.

---

## Chunk 5 — original items

25. **`DATABASE_URL` fallback** — `backend/src/config.rs:23-25`,
    `code_intel.rs:41-43`, `retrieval.rs:83-84` all do
    `SULION_DB_URL.or_else(DATABASE_URL)`. Every deployment sets `SULION_DB_URL`;
    e2e uses `SULION_E2E_DB_URL`, integration tests `SULION_TEST_DB`. Nothing sets
    bare `DATABASE_URL` for these processes.
    **Do not touch** `container_runner/postgres.rs:791-792`, which injects
    `DATABASE_URL` into user dev containers — that is a feature.
26. **`"worktree"` accepted as an alias for workspace mode `"isolated"`** —
    `api/session_launch.rs:255,280,286`, `api/session_routes.rs:202`,
    `node_runtime/mod.rs:301`, while the error text says "must be one of: main,
    isolated" and the frontend type is `"main" | "isolated"`
    (`frontend/src/api/types.ts:153`). Low residual risk from stale browser tabs
    since no shipped frontend sends it. Distinct from workspace *kind*
    `'worktree'`, a real persisted value — keep that.
27. **Health-check strings unreachable in production** — `api/mod.rs:84-90`
    reports `role: "standalone"` / `development_node: "local"`, but real
    standalone reports `"control-plane"`. Fold into item 31 rather than fixing
    separately.

---

## Chunk 6 — Cross-component legacy cruft (two-phase; no dependencies)

Each of these is a contract between independently-released components. None can
be a single-release delete. The pattern for all of them: **stop writing the old
form in release N, keep reading it; remove the reader in release N+1, once no
live peer can still emit the old form.**

Note these are *not* node-protocol contracts and do not depend on Chunk 1 — the
peers here are the correlate unix socket (item 28), a browser tab holding stale
JS (item 29), and the database as written by an older backend instance (item 30).
What each needs is release sequencing, not version negotiation.

28. **`claude_session_uuid` serde alias** — `backend/src/correlate.rs:49`, with
    `#[serde(default = "default_agent")]` at `:51` and `default_agent` at `:139`.
    Sender is `backend/hooks/session-start.sh:42`, baked into the image at
    `backend/Dockerfile:270`. The hook executes **on the node**; the correlate
    receiver is **in the control plane**. Same image, independent deploy timers,
    so this is an ordering-dependent contract.
    *Phase A:* change the hook to send `{"pty_id","session_uuid","agent":"claude-code"}`.
    *Phase B (later release):* drop the alias, the default, and the module comment
    at `correlate.rs:13-14`; update `tests/correlate_integration.rs:293` and the
    unit test at `correlate.rs:642`.
29. **`claude_resume_uuid` request alias** — `api/session_routes.rs:168-170`,
    consumed at `api/session_launch.rs:28,108,125`
    (`req.resume_session_uuid.or(req.claude_resume_uuid)`), with the paired
    agent-inference default at `:29-32` and `:126-129`. The peer here is a
    **browser tab**, which can hold stale JS indefinitely — an unbounded gap.
    *Phase A:* confirm the frontend sends only the new fields
    (`frontend/src/hooks/useResumeSession.ts:40-41` already sends both).
    *Phase B (later release):* drop the alias; the `(None, Some(_), None, None)`
    arm in `resolve_session_launch` collapses into the only no-agent case.
    **Not cruft:** the `"claude"` alias in `agent.rs:24` — the frontend actively
    sends `"claude"` as a launch agent id.
30. **`pty_sessions.current_claude_session_uuid`** — written at
    `correlate.rs:373` and `:387-390` via `CASE WHEN $3 = 'claude-code'`; single
    reader is the `p_reverse` lateral join at `metrics.rs:227`. Equivalent data
    lives in `current_session_uuid` + `current_session_agent`, maintained in the
    same UPDATE. Persisted state, so the column drop must not race a still-running
    old backend that writes it.
    *Phase A:* rewrite the metrics join onto `current_session_uuid`; stop writing
    the column; update `tests/correlate_integration.rs:250-258`.
    *Phase B (later release):* migration dropping the column.

---

## Chunk 7 — Remove the dual-runtime flag (done)

Item 31 applied and item 27 folded in. ~810 net lines removed. Full suite green:
11 targets, 110 tests, plus clippy clean and 215 unit tests.

**What the flag actually was.** `main.rs` already passed `true` for every role
including standalone, so the `false` branch was unreachable by configuration —
only by editing source and rebuilding. Single-machine deployment does not
depend on it and is unaffected: `SULION_DEPLOYMENT_ROLE=standalone` selects
loopback transport, and `main.rs` still builds a `NodeRuntime` and connects it
in-process. `make validate-deploy` passes for all four compose variants.

**What removing it exposed.** The whole local implementation was dead once the
branch went: `resolve_session_launch`, `resume_agent_launch`,
`default_agent_launch`, `resolve_session_workspace`, `repo_path_for_session`,
`pty_workspace_metadata`, `SessionLaunch`, the local websocket handler
`handle_socket`, the shell-command builders in `session_routes`,
`file_content::serve_bytes` with `RAW_MAX_BYTES`, and `workspace::read_file`.
The last one was invisible to the dead-code lint because it is `pub` in a
library crate — `pub` items need a manual reference check, which is the same
blind spot that hid Chunk 3's findings.

**Seven real bugs on the shipping path**, all previously masked because the
tests exercised the legacy path and the node path had no coverage:

- A missing file returned 503, not 404: `raw_file` let the not-found error fall
  into `RuntimeError::Internal`, which the proxy reports as "development node
  unavailable".
- A rejected path on upload returned 503, not 400, from the same generic
  conversion in `RepoUpload` / `WorkspaceUpload`.
- A rejected path on read returned 404, not 400 — **self-inflicted**: the first
  fix above mapped every `read_file` failure to `NotFound`, collapsing
  path-safety rejections into ordinary misses. `raw_file` now separates all
  three outcomes explicitly.
- Workspace deletion refusals returned 503, not 400. Live sessions, uncommitted
  work, and unmerged commits are all things the caller can act on.
- **A session launch could fail outright the first time a repo was used.** A
  node's periodic workspace sync and `ensure_main_workspace_owned` both
  select-then-insert the main workspace, and `workspaces.path` is unique, so the
  loser of that race failed its insert and took the launch down with it. The
  insert now converges with `ON CONFLICT (path) DO UPDATE ... RETURNING id`.
  This one affects standalone in production; it stayed hidden because a
  legacy-mode `AppState` never started the sync loops.
- `protocol_working_dir` hardcoded `/home/sulion/repos`, so the control plane
  asserted one node's disk layout from a string literal and rejected any other
  root. It now uses the configured `repos_root` — identical in production.
- `resolve_in_repo` returned the repo root itself with a trailing separator,
  because `Path::join("")` appends one, so a `working_dir` equal to the repo
  root compared unequal to every other spelling of the same directory.

**Test migration was not a constructor swap.** `rest_integration`,
`ws_integration`, `workspace_integration`, and `device_integration` now build
an `AppState` with a loopback `NodeRuntime` via `tests/common/mod.rs`, so what
they exercise is what ships. Two fixtures had to change because they relied on
local-path behaviour: a repo created with a bare `mkdir` has no `repos` row, and
repo routes resolve the owning node from that row. `device_integration` now
creates repos through the API; `common::register_repo` covers tests that
genuinely need an on-disk repo, mirroring the node's
`claim_discovered_resources` step.

**Harness guardrails.** The suite runs in ~110s; the rest of the wall clock is
the optimised build. A run that exceeds `SULION_TEST_TIMEOUT_SECONDS` (default
1200) is now killed by `timeout` against the whole process group and says why,
because the failure mode is silent: a test that leaves a PTY alive has that
child inherit the test binary's stdout, so the pipe never closes and the runner
waits on a test that already finished. That is what made runs look like they
took 18 minutes. Cleanup now goes through the node — `state.pty` owns nothing a
node spawned — and `stdin` is closed so nothing can block on input.

**Known rough edges.**

- A node claims repos it discovers only at startup, so a repo added to
  `~/repos` by hand is not routable until the next restart. Pre-existing and not
  a regression: standalone already ran with the protocol required. Candidate for
  Chunk 10.
- `history_returns_events_after_ingest_and_correlate` failed once in a full
  suite run and passed standalone and on re-run. Suspect the 2-second background
  workspace sync racing session creation. Its blind `unwrap` on the response has
  been replaced with a status assertion, so a recurrence will name the cause
  instead of reporting "unwrap on None".

---

## Chunk 7 — original item

31. **Delete `node_protocol_required = false`.** `lib.rs:85-88` calls the legacy
    managers a compatibility/rollback path, but `main.rs:51-59` hardcodes `true`
    for every deployment role including standalone, which runs the loopback
    `NodeRuntime` with the protocol still required. The `false` path is reachable
    only via `AppState::new` / `new_with_auth` in tests. The documented rollback
    is the standalone *compose overlay*, which uses loopback — not this flag.

    ~35 branch points: `api/session_routes.rs:187,300,334,371,414`; thirteen in
    `api/workspace_routes.rs`; twelve in `api/repo_routes.rs`;
    `api/repo_lifecycle_routes.rs:43,81`; `api/ws.rs:106,172`; `api/stats.rs:98`;
    and the unreachable health strings at `api/mod.rs:84-90` (item 27).

    *Order:* migrate `tests/rest_integration.rs`, `ws_integration.rs`,
    `workspace_integration.rs`, `device_integration.rs` onto a loopback
    `NodeRuntime` first; then delete the flag and the local arms; then collapse
    the `new` / `new_with_auth` / `new_with_auth_and_node_mode` constructor chain.

---

## Chunk 8 — Layering and the data layer

32. **Extract a data layer for repo lifecycle.** `db.rs` is connection plumbing
    only; raw SQL lives in 47 files. Worst is `api/repo_lifecycle_routes.rs` with
    25 queries, including cross-table renames of tables owned by other modules:
    `:388` `timeline_file_touches`, `:412-422`
    `retrieval_embeddings`/`_sources`/`_backfills`. A schema change in ingest or
    retrieval breaks the rename path silently. `api/device_routes.rs` has nine
    more. The intended pattern already exists — `plans.rs` as the data module with
    `api/plan_routes.rs` as a 172-line pass-through.
33. **Fix the inverted dependency in the same change.**
    `node_runtime/requests.rs:152,167` calls
    `crate::api::repo_lifecycle_routes::{rename,delete}_repo_runtime` and
    `:220,320` calls `crate::api::file_content::build_preview`; `api/mod.rs:17,22`
    widened visibility to `pub(crate)` purely to permit it. Moving the
    repo-lifecycle SQL and `build_preview` into domain modules resolves items 32
    and 33 together. Then extend `backend/tests/structure_lint.rs` (which already
    lints size caps) to deny `crate::api` imports outside `api/`.
34. **Broker per-tool grant model** — prerequisite for retiring
    `LEGACY_SECRET_GRANT_TOOLS` on the frontend (see Chunk 3). Note this is itself
    a cross-component contract with the browser; same two-phase rule as Chunk 6.
35. **Minor boundary leak** — `api/session_routes.rs:21` and
    `api/timeline_routes.rs:12` import `ingest::{canonical, timeline}` directly;
    `canonical` is the ingester's internal JSONL parse model. The rest of `api/`
    uses the flat `ingest::load_*` re-exports and never touches
    `ingest::projection`. Add facade re-exports for the few types these two need.
36. **Small duplications, opportunistic.** `env_required`/`env_optional`
    re-declared eight times with three different semantics (`retrieval.rs:148-162`,
    `code_intel.rs:136-151`, `container_runner.rs:67-72`,
    `code_intel/indexer.rs:620-625`, `api/admin_routes.rs:157-163`, both CLIs).
    Four service binaries (`sulion_broker`, `sulion_retrieval`,
    `sulion_code_intel`, `sulion_runner`, 21-28 lines each) are the same
    tracing+config+serve program. `bin/e2e_seed.rs` relies on autodiscovery rather
    than an explicit `[[bin]]` stanza.

---

## Chunk 9 — Docs and nix janitorial (cheap)

37. **Delete `docs/code-intel-findings.md`.** A spent June incident artifact that
    now contradicts current docs: it calls the parser-hang fix "Not yet
    implemented" and persistent LSP "Architectural"/future, but both shipped
    (`code_intel/lsp.rs:28-69,284` with idle eviction, documented at
    `docs/deploy.md:174-183`; `parser.rs:8` caps files at 2 MB). Nothing
    references it. Fold any live remainder into `docs/backlog.md`.
38. **Document two undocumented features.** Future prompts
    (`future_prompts.rs` 409 lines + `api/future_prompt_routes.rs` 151 +
    `FuturePromptsModal.tsx`) has no mention anywhere in `docs/`. Device pairing
    (`api/device_routes.rs` 524 lines + `PairPage.tsx` + the `/pair` approval
    flow) is documented only in a compose comment — and it is an auth surface, so
    it belongs in `docs/architecture.md`.
39. **Retire `nix/repair-existing-install.md`** once the one dedicated machine is
    confirmed on the fresh-install/flake path. It is a one-shot transition doc for
    hosts still running from the retired root-owned `/etc/sulion` checkout — the
    same era as the `repository-cutover.md` already deleted in the working tree.
    Remove the link at `nix/README.md:435`. Keep `sulion-admin-key` /
    `install-admin-key`, which are ongoing rotation.
40. **Give `sulion-stack-adopt` a sunset condition.**
    `nix/modules/sulion-deployer.nix:23-44` removes containers from a previous
    generation's compose project name and runs as `ExecStartPre` on *every* stack
    start (`:298`). It describes itself as "a migration, not a teardown." Either
    remove it or comment the condition under which it can go.
41. **Compose crumbs.** `deploy/compose.truenas.yaml` re-sets
    `SULION_DEPLOYMENT_ROLE` / `SULION_NODE_TRANSPORT` identically to the
    `compose.standalone.yaml` it is always paired with; only
    `SULION_DOCKER_MODE: brokered` is real policy. Three Makefile assertions plus
    `nix/tests/dev-node-vm.nix:162` still assert the removed `node-tunnel` service
    is absent — fine as regression guards, worth a dated comment.
44. **Restore the gateway header rationale.** `node_protocol/gateway.rs:70-79`
    forwards eight header names, five of them `x-sulion-*` identity-ish, and the
    comment explaining why that is safe was dropped when the list grew. Both
    consumers — `retrieval/context.rs:34-80` and `code_intel/api/root.rs:82-125` —
    use these purely as scoping defaults and already accept the identical values
    as query parameters, so spoofing them gains nothing. Write that down at the
    call site, because the invariant is load-bearing: if either service ever
    treats `x-sulion-pty-id` as *identity* rather than a default, this becomes a
    straightforward spoof from anything on the node LAN.

---

## Chunk 10 — Decisions needed (not code changes yet)

42. **`repos` vs `repo_runtime_state` split-brain.** `metrics.rs:336-337` asserts
    "repo_runtime_state is the live registry (the 0001 `repos` table is legacy and
    stale in deployed databases)" — yet `repos` is still written at
    `node_runtime/mod.rs:250`, `node_runtime/requests.rs:385`, and
    `api/repo_lifecycle_routes.rs:308-319,474`, and `repos.node_id` is
    load-bearing for node routing at `api/node_proxy.rs:28` (column added in
    `migrations/0059_dev_node.sql:62`). Either the comment overstates or node
    ownership belongs on `repo_runtime_state`. Given only `DEDICATED_NODE_ID` can
    ever pair (`node_protocol/store.rs:30`), per-repo node ownership may not need
    to exist at all.
43. **Completed one-shot usage backfill.** `ingest/usage_backfill.rs` (marker
    `"claude-usage-dedup-v1"`, line 20) is spawned unconditionally at
    `main.rs:101` and is a marker-gated no-op; the deployed DB finished it in the
    0056 era. Before deleting, confirm the marker row exists in the production
    `usage_backfills` table — otherwise removal silently un-runs a migration that
    never completed.

---

## Verified current — explicitly not cleanup targets

Recorded so these are not re-flagged by a future review:

- **Standalone/loopback transport** (`config.rs:92-119`) — the live generic-Linux
  rollback path with real compose overlays. All four compose variants are
  exercised (`make validate-deploy`, `platform.yml:14`,
  `nix/modules/sulion-deployer.nix:17`).
- **`node_id IS NULL` husk handling** (`api/session_routes.rs:414-436`,
  `api/node_proxy.rs:24`, `main.rs:39-45`, `pty/mod.rs:62`) — pre-node rows
  genuinely persist in the deployed DB; shipped in 22e3bb4.
- **`canonical_backfill.rs`** — version-gated durable maintenance by design.
- **Migrations 0064-0066** — correct one-time data rewrites.
- **All six architecture invariants** verified upheld in code.
- **`scripts/`** — all four referenced from Makefile, CI, or playwright config.
- **`infrastructure/terraform`** — applied by the shared CI deploy action, state
  at `projects/sulion.tfstate`.
- **Module layout** (`foo.rs` + `foo/`) — consistent across all seven pairs, no
  upward imports from `ingest/` or `pty/`.
