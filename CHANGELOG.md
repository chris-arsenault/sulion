# Changelog

All notable user-visible changes to Sulion are recorded here.

## Unreleased

### Transcript compatibility

- Preserve queued Claude prompts and pasted text, reconcile submitted prompts,
  and replace partial response usage with the latest receipt.
- Keep Codex child lineage intact across inherited history, display agent
  messages, link current subagent-start records, and enrich tool operations with
  completed runtime evidence without double-counting operations.
- Repair retained canonical, timeline, and usage projections on startup and
  reindex while preserving raw transcripts and archived digests.

### Transcript archive

- Added a monthly archive cycle in the control process. Idle agent sessions
  are exported from the database to an S3 bucket under the backend's own
  machine identity, the durable tables are dumped beside them, and after a
  grace period each exported session is purged down to its turn digest:
  prompt, markdown, timestamps, tokens, and the files each turn touched.
  Cost and file churn are rolled up to daily repo-level tables first, so the
  metrics do not change at the purge. `sulion-retrieve search`, `turn`, and
  `file-history` keep answering for archived sessions; the timeline shows
  their turns from markdown with an archived banner. `sulion archive
  run|restore|status|list` and `/api/admin/archive*` queue work for the
  loop; a restore replays the archive through the normal ingest path and
  can purge again afterwards, which is how a whole-history re-index runs.
  Deletion is behind an operator gate that starts closed: the first cycle
  exports and dumps only, `sulion archive verify --deep` re-reads every
  object, and `sulion archive purge-gate on` is what allows purging.

### Retrieval

- Made pgvector a requirement and dropped the `REAL[]` copy of every
  embedding. The extension, vector column, and HNSW index are created by
  migration; the service verifies them at startup instead of creating them
  lazily, and the exact-scan fallback is removed. Halves the embeddings
  table's row payload. The integration harness and e2e stack run the
  `pgvector/pgvector:pg16` image.

### Timeline

- Moved timeline filters, text size, and List/Grid/Hidden navigation into a
  shared settings flyout above the prompt input. Changing a setting now
  updates every open timeline and survives reloads. Grid navigation opens
  its own flyout with larger turn cells; terminal text size remains separate.
- Added per-turn duration and token metrics and preserved the reader's
  position while detail updates. Selecting an older turn disables
  follow-latest, so incoming work does not pull the selection back.
- Added Claude child-agent transcript ingestion and fixed concurrent child
  turns overwriting one another when their byte offsets coincided. Child
  logs stay associated with the parent delegation, and bookkeeping records
  no longer manufacture promptless turns. Codex response-usage records and
  Claude latch records are hidden from ordinary timeline reading.
- Changed Codex code-mode operations to show the nested command or tool
  instead of an opaque `exec` wrapper. Native patches retain their edit
  evidence, and operation categories follow the underlying work.
- Added paste-as-file to the timeline composer for large text and clipboard
  images, matching the terminal upload workflow. The resulting file
  reference stays in the session's workspace.
- Added a model-switch guard. Codex and Claude Code can move a session onto
  a different model on their own — Codex by applying new thread settings
  between turns, Claude Code by falling back mid-turn — and the timeline
  showed it only as a metadata line. The ingester now records every model
  change against the model the session launched with or the user last
  accepted; a change away from it stops the turn running under the new
  model and opens a confirmation dialog in the timeline pane. The dialog
  shows the models, effort, when the change was observed, whether a turn
  was interrupted, and the evidence the transcript holds near the switch:
  Codex's last rate-limit snapshot, or Claude's fallback block and request
  iterations, since neither harness writes a reason. *Continue* accepts the
  new model; *Dismiss* keeps the previous one expected, so restoring it by
  hand is recorded without being enforced.
- Changed the turn digest that `sulion-retrieve turn` and "copy turn as
  markdown" return. It now holds the prompt, the assistant's text, and one
  header line per tool call; tool inputs, reconstructed diffs, and full
  result bodies are no longer embedded. Those live in the operation rows
  and event blocks the turn-detail view and search already read, and
  embedding them made the average digest 48 kB and a long turn's over a
  megabyte, rewritten every time the turn grew. Stored digests are
  re-rendered on the next control start.
- Fixed a long-running turn rewriting megabytes per event. The ingester
  re-upserts a live turn's row on every tick that appends to it, and the
  row carried a whole-turn JSON copy that nothing read, next to the
  regenerated markdown. A 2 MB turn rewrote about 3.5 MB of TOAST per
  event, the largest write-ahead-log source in the system. The unread
  column is dropped, and a session that keeps growing is now projected at
  most once every 5 seconds; a session that stops growing is projected on
  the next tick, so a finished turn still shows within a second.
- Fixed the repo and workspace status pollers rewriting the database every
  30 seconds whether or not anything changed. Each cycle deleted and
  reinserted the dirty-path rows and updated the status row two or three
  times through an indexed column, which on a quiet system produced tens
  of gigabytes of write-ahead log a day, bloated a 55-row table to 184 MB,
  and kept autovacuum running continuously. A cycle whose `git status`
  fingerprint matches the stored one now writes only its next due time,
  and the schedule indexes are dropped so that write stays heap-only.
  Reclaiming the existing bloat needs a one-time `VACUUM FULL` on
  `repo_runtime_state` and `workspaces`.
- Fixed shell and script tokens being recorded as touched files. The
  file-touch extractor split a tool's command text on whitespace and kept
  any token containing a dot or slash, so `2>/dev/null`, `*.cs`,
  `PYTHONPATH=.`, and every line of an inline Python script became a file
  row, and a code-mode exec that applied a patch flagged them all as
  writes. Roughly 60% of the stored touch rows were such noise, padding
  turn detail, retrieval evidence, and the Metrics churn hotspots. A
  command snippet is no longer tokenised once the canonicaliser has
  produced its structured edit list, and a bare token must be path-shaped.
  Existing sessions are reprojected on the next control start.
- Fixed an interrupted Codex turn leaving the session reported as working.
  The `turn_aborted` record now resolves activity to awaiting-prompt like a
  completed turn does.
- Fixed synthetic API-error records overwriting a Claude session's reported
  model with the `<synthetic>` placeholder.
- Fixed markdown links in turn detail navigating the browser. A relative
  link such as `[brief](docs/BRIEF.md)` resolved against the page origin,
  which Sulion does not serve, and unloaded the app. Such links now open a
  Sulion file tab for the session's repo and workspace, honouring a
  trailing `:line` or `#L12` reference; absolute checkout paths are trimmed
  to the repo. External `http(s)` links open in a new browser tab, and a
  link with no repo context or an unsafe scheme renders as plain text.
- Fixed library prompts and queued future prompts vanishing in timeline-only
  mode. The terminal pane stays mounted but hidden there, and injected text
  was pasted into it unseen. Injection now lands in the timeline prompt box
  whenever the timeline is the only projection on screen, including mobile,
  and in the terminal in split and terminal-only modes. The library also
  resolves the target session from an active timeline tab, so a session
  with no terminal tab open can still receive a prompt.
- Added TeX math rendering in turn detail through KaTeX. Codex and ChatGPT
  emit formulas with the LaTeX `\[ … \]` and `\( … \)` delimiters, which
  markdown previously reduced to a bare `[` and mangled source; those and
  the `$$ … $$` form now render as display or inline math. Single-dollar
  `$x$` is left as text so `$HOME`-style prose is never paired into a
  formula, and delimiters inside code spans and fences stay literal.
- Added a record of every prompt sent from the timeline input, written
  before the text reaches the terminal and matched to the transcript turn it
  becomes. A new Submitted Prompts window, opened from the prompt bar, lists
  recent sends with their matched or unmatched state and offers copy, retry,
  and dismiss, so a prompt a harness startup dialog swallowed is not lost.
- Changed the timeline prompt box to close while a freshly launched harness
  has not yet reported its session, or while the agent waits on a terminal
  question, with a pointer to the terminal view and a "Type anyway" escape
  hatch. The prompt route refuses the same cases unless the send is forced.
- Fixed Codex correlation on Codex 0.154, which holds its rollout file open
  only from the first turn. The launcher now also reads the per-session
  thread-writer lock Codex opens at startup, so a Codex launch is recognised
  before any prompt is sent instead of after it.

### PTY toolset

- Fixed existing shells retaining an old Sulion CLI after a node release.
  The node now delivers the CLI on every start, even when it adopts an
  unchanged toolset container. Its stable symlink is replaced atomically,
  so the next command uses the new release without restarting the shell.
- Fixed direct Docker access in devenv shells by forwarding the host socket
  and its numeric group to the container. Added the CA-bundle path expected
  by Nix tools to both workbench images.
- Changed Claude Code to run from a native install in the persistent home,
  seeded from the image on first start, instead of the root-owned npm tree.
  Its built-in updater now upgrades PTY sessions without `sudo`, and the
  upgraded copy survives image rolls. The entrypoint also repairs a stale
  `~/.npmrc` prefix and a dangling `~/.local/bin/claude` link left behind by
  a home-directory move.

### Dedicated development node

- Changed secret-bearing TrueNAS services to read their database URL and
  service tokens at startup under their own workload identities. Deployment
  resolves public Cognito identifiers; dedicated-node credentials still
  arrive over the authenticated node channel. Credential redelivery no
  longer sends an already approved node through enrollment again.
- Restricted host SSH to the trust management appliance at
  `192.168.67.2/32`. Secure LAN and server-subnet clients retain SMB and
  development-port access but can no longer open direct SSH sessions.

### Plans

- Added branch plans: a published plan can now hang off one or more phases of
  another plan, with nesting capped at depth 8. `sulion plan branch` opens a
  sub-plan under the phases it covers and moves the terminal onto it; `sulion plan return`
  closes the sub-plan, puts the terminal back on the parent, and clears any
  anchor phase the branch was opened to unblock. `sulion plan tree` prints a
  whole plan tree. A plan refuses to close while a branch under it is open.
- Changed the plan browser to nest branches under the plan they hang off, lead
  a branch with a clickable trail back to its root, list sub-plans beneath the
  phase they cover, and offer Return/Abandon in place of Complete/Cancel. Each
  phase gained a control that opens a sub-plan under it.
- Changed plan flow metrics to read leaf phases only and to draw one burndown
  per plan tree rather than one per plan, so a sub-plan no longer double-counts
  the phase it sits under or displaces its own parent from the chart.

### Workspace and sessions

- Added split, terminal-only, and timeline-only desktop display modes, with
  shortcuts for cycling modes, toggling the sidebar, and peeking at the
  hidden terminal or timeline. Tabs render once into stable hosts, so pane
  moves and peeking preserve terminal connections and scrollback.
- Changed mobile to a timeline-only workspace with drawer navigation and
  reachable file/reference tabs. Opening a session no longer creates an
  invisible terminal tab, and mobile does not overwrite the desktop display
  preference. Fixed viewport height and touch interactions around the
  timeline and prompt controls.
- Added metadata-only repository groups and collection sessions. A group
  has one primary repository, uses canonical checkouts, and passes the
  other member roots to Claude or Codex as additional directories. Sidebar,
  rail, palette, and Overview group the work without moving repositories or
  allocating member worktrees. Group edits affect later launches and resumes;
  retrieval and file operations retain their existing repository scope.
- Added Fugu as a launchable, resumable agent identity using the
  `codex-fugu` profile wrapper. It now correlates with its PTY and appears
  in the timeline instead of leaving an orphaned Codex transcript.
- Fixed repository rename racing background discovery and retired the old
  code-intelligence root during rename, so the previous name no longer
  reappears as a second repository or index root.

### Library and secrets

- Moved saved prompts, references, and queued future prompts into Postgres.
  The control API now sees the same entries across deployment topologies
  instead of depending on node-local markdown directories.
- Added multiline secret values, preserving embedded and trailing newlines
  through storage and credential redemption. Fixed delivered secret-file
  ownership so the service user can read files written during bootstrap.

### Metrics and background work

- Replaced ambiguous token totals with separate input, cached-input, and
  output categories, daily usage, and per-model API cost estimates. Cache
  writes retain their provider-specific rates; unknown prices leave totals
  visibly incomplete. Estimates use the displayed catalog rates and do not
  claim to reproduce subscription charges or historical invoices.
- Corrected Codex usage accounting to deduplicate per-response usage,
  including compaction, while preserving the legacy cumulative prefix of
  older sessions. Context snapshots after that transition update context
  pressure without counting spend twice. Historical response sessions are
  repaired from stored events on startup.
- Added background-job progress and recent outcomes to Metrics. Transcript
  catch-up and projection repairs now expose work counts and stalled,
  interrupted, or failed state instead of appearing as an unexplained wait.
- Split startup maintenance into independently versioned canonical,
  timeline, and usage repairs. A timeline change no longer forces an
  unrelated usage rebuild; failed canonical rows remain eligible for retry.
- Reduced database contention by preserving unchanged projected rows and
  operation embeddings, batching embedding writes, and moving repeated
  vector schema setup out of the indexing loop. Git activity is materialized
  by the node, database inventory runs outside the frequent stats sample,
  and app-state polling uses bounded refresh work.

## v2.1.0 - 2026-08-03

### Session lifetime and toolsets

- Moved PTY masters and shadow emulators into a devenv server. On the
  dedicated node, its containers survive node releases and reconnect with
  their running shells; the standalone and test roles use the same server
  as a child process and retain their existing lifetime boundary.
- Added versioned toolset containers keyed by image identity. Existing
  sessions keep their toolset while new sessions use the current image.
  The explicit per-session upgrade restarts only that shell, preserves its
  identity and workspace, and leaves neighboring sessions alone. Empty
  non-current containers are reaped.
- Fixed missing or disconnected devenv sessions remaining falsely live,
  and corrected resume path splitting so a repository name repeated in a
  parent directory does not select the wrong checkout.

### Timeline

- Changed Claude background-task notifications to stay inside their owning
  primary turn instead of appearing as standalone prompt rows in the timeline
  rail. The notification detail remains available inside the turn, and startup
  automatically repairs affected historical projections without changing the
  canonical transcript data.

### Secrets

- Reworked the Secrets tab into a pane-responsive bundle workspace with a
  compact toolbar and bundle count, independently scrolling bundle list and
  editor, and a pinned Save/Delete footer that remains reachable in split and
  narrow layouts. Narrow panes switch the bundle list to a horizontal strip
  while preserving the existing create, edit, and delete behavior.

## v2.0.0 - 2026-07-29

### Dedicated development node

- Added self-enrolling development nodes. A node boots holding nothing but its
  Ed25519 identity key, waits for a single **Approve node** press in the stats
  panel, and receives its database credentials, retrieval token, and broker
  registration token over that authenticated channel. Installing a node no
  longer involves writing an environment file or copying credentials between
  machines.
- Added on-machine generation of the host half of the node environment,
  including a code intelligence token that never leaves it, and resolved the
  current release at boot instead of pinning it by hand.
- Added control-plane authentication to the handshake, so a node authenticates
  control rather than only the reverse. Control signs with its own identity, and
  a node records that identity the first time it pairs and refuses every later
  connection that cannot sign for it — including one that simply omits the
  signature. Recovering from a legitimately replaced control plane means
  deleting the pin deliberately.
- Added signing and connection-binding for delivered configuration, with the
  node enforcing the forwarded-key allowlist on receipt rather than trusting the
  sender to have applied it.
- Changed node traffic to stay on the LAN and be encrypted end to end. Nodes
  reach a single TLS endpoint — control channel, broker, and retrieval — whose
  certificate is generated by the control plane, pinned by the node on first
  pairing, and bound into the signed handshake, so session credentials never
  cross the network in the clear and a substituted endpoint is refused before
  any protocol byte flows. TLS terminates in the control process itself: no
  kernel tunnels and no elevated privileges anywhere. The public reverse proxy
  returns 404 for the node channel, the control plane refuses node connections
  from outside the node LAN, and the build fails if any node destination points
  at the public hostname. Approving a node remains available from anywhere.
- Changed the node protocol to additive-only compatibility, so the control plane
  and a node may deploy in either order and an added payload field never severs
  a peer that has not upgraded yet.
- Fixed `sulion-stack.service` skipping silently when configuration was missing,
  which left a freshly installed host inert with only an unmet-condition line in
  the journal.

### Runtime identity

- Retired the image's portable `dev` account and every `/home/dev` path. UID
  7321 is `sulion` with home `/home/sulion` in the image, in every deployment
  role, and in every stored path; a data migration rewrites rows from the
  `/home/dev` era so no query or tool ever branches on which era wrote a path.
  PTY prompts, file ownership, and the host login now all agree.

### Sessions

- Changed session resume to move a conversation rather than copy it. Resuming in
  a new PTY releases the session from the PTY that previously held it, so the
  sidebar lists it once instead of showing the dead shell beside the live one. A
  data migration releases bindings duplicated before the fix, including
  ingester-discovered sessions that never had an authoritative PTY link; every
  session keeps only its most recent claimant.
- Changed husk deletion to stop requiring the development node to be reachable.
  Sessions from the legacy local runtime, or from a node identity that no longer
  exists, have no process anywhere, so DELETE removes the row directly; only
  live sessions still demand their owning node. This unblocks **Resume with new
  PTY**, which replaces the husk it resumes from and previously left it stranded
  beside the successor.

### Plans and overview

- Added durable repo-scoped published plans with ordered phases, status notes,
  PTY attachments, revisioning, and audit history. Plans are available through
  the browser and the new `sulion plan` PTY CLI without replacing agents'
  detailed internal plans.
- Added agent-operational activity reporting through lifecycle hooks,
  transcript signals, and `sulion activity`, including explicit needs-input
  and blocked states.
- Reworked Monitor into a manager-style Overview: repos are teams, live
  terminals are engineer cards, attention states sort first, and every card
  combines activity, current plan/phase, latest output, uptime, cache-aware
  token spend/rate, and measured context remaining.

### Monitoring

- Changed the sidebar's memory and CPU figures to report the development node's
  whole machine instead of the control-plane process. Under a split deployment
  the control plane runs no PTY, build, or language server, so its own figures
  described the one host never under pressure. The node samples itself and
  reports it on each heartbeat; with no node connected the strip shows dashes
  rather than a number about the wrong machine.
- Changed dev-server guidance in the PTY toolset doc to point at the node's own
  LAN address instead of the control plane's, and to say what happens outside
  the published `26000-26010` range.

### Bug fixes

- Fixed SVG file previews to render through the image viewer instead of
  inlining repo markup into the page.
- Fixed unbounded terminal resize, which could end every live terminal on a
  node.
- Fixed the ingester re-reading an unterminated transcript line every tick, and
  timeline previews failing on multi-byte prompt text.

## v1.6.0 - 2026-07-09

### Retrieval

- Changed semantic indexing to embed only natural-language content —
  assistant/user/summary text, subagent finals, capped tool-call inputs, and
  tool errors — chunked instead of truncated. Command output, file
  reads/writes, diffs, and image payloads are no longer embedded, roughly
  halving embedding volume; search dedups to the best chunk per source.
- Excluded low-value tool operations (bash, file edits/reads, grep, etc.)
  from search results by default, with an `--include-low-value` override.
- Added `POST /v1/index/reset` and `sulion-retrieve reset --confirm` to
  rebuild the semantic index from scratch without touching transcript text.
- Fixed semantic indexing stalls by splitting embedding requests to the
  embedding server's maximum client batch size.
- Fixed lexical search to hit the partial trigram index instead of
  seq-scanning `event_blocks` (~49s to ~200ms for scope=all queries).
- Made the background semantic indexer disableable via
  `SULION_RETRIEVAL_INDEX_SECONDS=0` to protect the shared embedding server.

### Code intelligence

- Added persistent LSP sessions with rust-analyzer and
  typescript-language-server installed in the code-intel image, so `def`/`refs`
  resolve semantically instead of falling back to the syntactic index.
- Fixed the code-intel container to run as the data-owner uid so the indexer
  can read the repos/workspaces datasets under their NFSv4 ACLs.
- Changed indexing and structural search to respect `.gitignore` and skip
  common generated/vendor directories (build, dist, caches, venvs).
- Fixed the O(files×symbols) status query (~3s to ~2ms) and corrected
  `sulion-code` pack hints for semantic search result ids.

### Repo management and login

- Added repo rename and delete actions to the sidebar repo context menu,
  backed by new repo lifecycle API routes with live-session protection.
- Added Cognito software-token (TOTP) MFA support to the login form. Sign-in
  now completes the `SOFTWARE_TOKEN_MFA` challenge with a code-entry screen;
  un-enrolled users are directed to the central enrollment app.

### Bug fixes

- Fixed terminal pane control and auth bar layout issues.

### Toolchain

- Added Ruby via RVM to the PTY base image.

## v1.5.0 - 2026-06-15

### Device pairing and external ingest

- Added OAuth-style device-authorization pairing (`POST /api/devices/pair`,
  `/pair/token`, browser `/pair` approval page) so external tools can obtain a
  Sulion device token. Only secret hashes are stored, and approval happens
  inside the existing authenticated UI.
- Added device-token-authed repo file write via
  `POST /api/repos/:name/ingest?path=<repo-relative>` — the HTTP analogue of
  paste-as-file. Raw request bytes are written through the existing
  path-safety layer (no traversal, symlink escape, or absolute paths). The
  first consumer is the Ableton "Send to Sulion" extension; an earlier
  MIDI-specific ingest table was replaced by this generic contract.
- Added device-token-authed raw file download via
  `GET /api/repos/:name/raw?path=<repo-relative>` so paired tools can read
  binary repo content back (e.g. pulling clips into Ableton Live).
- Fixed the pairing `verification_uri` to derive from the request's forwarded
  host headers instead of a hard-coded localhost default, with
  `SULION_PUBLIC_URL` pinned in compose as the deterministic override.

### Bug fixes

- Fixed Codex PTY binding to only consider rollout files the process has open
  for writing, so browsing resume/history no longer rebinds a PTY to an
  unrelated repo's session.
- Fixed agent runtime "running" updates to retry when they race PTY session
  creation.

### Toolchain and testing

- Added `sulion postgres -- <command>`: a managed, workspace-scoped
  Postgres 16 test container with `DATABASE_URL`/`PG*` injection, reused
  across runs, plus `--restart` and `--temp` modes.
- Bumped the PTY image's Node from 20 to 24.

## v1.4.0 - 2026-06-01

### Agent retrieval

- Added the `retrieval` service and `sulion-retrieve` PTY helper for
  transcript/timeline search, turn lookup, file history, facets, and reindex
  actions from agent shells.
- Added lexical and semantic retrieval over existing Sulion Postgres data
  without duplicating transcript text into a separate projection. Semantic
  indexing stores embeddings plus source keys and uses the local embedding
  service at `192.168.66.3:5361`.
- Added optional `pgvector` acceleration for retrieval embeddings while keeping
  a `REAL[]` exact-scan fallback when the extension is unavailable.
- Added a UI-triggered retrieval backfill path through the backend admin route
  so embedding backfill can be kicked off without exposing the retrieval static
  token to the browser.

### Code intelligence

- Added the standalone `code-intel` service and `sulion-code` PTY helper for
  agent-facing structural source navigation.
- Added compact code index tables for roots, files, symbols, references,
  imports, and index jobs. The index stores structural facts and ranges, not
  full source text or serialized ASTs.
- Added Tree-sitter parsing, incremental refresh, normalized symbol extraction,
  lightweight syntactic references, ast-grep structural `search`, diff-only
  structural `patch`, LSP-backed `def`/`refs` escalation with syntactic
  fallback, and budgeted `pack` responses.
- Added the authenticated `/v1/help`, `/v1/status`, `/v1/refresh`,
  `/v1/outline`, `/v1/find`, `/v1/def`, `/v1/refs`, `/v1/search`, `/v1/patch`,
  and `/v1/pack` code-intelligence API routes.

### Deployment and tests

- Added `retrieval` and `code-intel` image, compose, platform, token, and PTY
  environment wiring. Repos and workspaces are mounted read-only into
  `code-intel`.
- Updated `make ci` to stay a fast lint/unit/typecheck gate; the Postgres-backed
  backend integration suite remains an explicit `make test-rust-integration`
  gate.
- Added backend unit and integration coverage for retrieval indexing/search,
  code-intelligence parsing/indexing/navigation/structural operations, PTY env
  forwarding, and the agent-facing CLI contracts.

## v1.3.0 - 2026-05-27

### Secrets and credential grants

- Changed secret grants from tool-scoped unlocks to PTY-scoped unlocks. Enabling a secret for a terminal now makes that env bundle available to both supported wrappers instead of requiring separate `with-cred` and `aws` grants.
- Updated the Secrets context menus and grant API to treat the wrapper name as runtime/audit context rather than part of the grant relationship, with compatibility handling for older brokers during rollout.
- Updated the `aws` wrapper so it no longer depends on a hard-coded `aws-default` secret id. It now redeems any enabled secret bundle containing `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`.

### Prompt library and workspace UX

- Added templated prompt support for saved prompt-library entries. Prompts using `$name` placeholders open a value dialog before terminal injection; `$$` sends a literal `$`.
- Added a tab context-menu action to close terminal and session-timeline tabs whose backing PTY/session association no longer exists.

### Timeline correlation fixes

- Fixed Codex session correlation so the launcher only considers the launched process's own rollout file handles, preventing nested subagent work from rebinding the parent PTY.
- Excluded Codex subagent child sessions from first-class repo timeline membership so delegated work stays visible inside its parent turn instead of taking over repo/session timeline bindings.

### Documentation and tests

- Updated secrets, architecture, user-guide, and PTY toolset docs for PTY-scoped grants, AWS-shaped secret redemption, prompt templates, and repo timeline subagent handling.
- Added backend coverage for Codex nested-child correlation and repo timeline subagent exclusion, plus frontend coverage for PTY-scoped grant compatibility, tab cleanup, and prompt template rendering.

## v1.2.0 - 2026-05-05

### Agent control and monitoring

- Added an Escape-based interrupt action beside the timeline prompt Send button so Codex/Claude sessions can be interrupted without opening the full terminal.
- Reworked the Monitor tab into a reading-first layout where recent assistant output owns the main space and user prompts are shown as compact context.

### Workspace management

- Added sidebar workspace management under each repo. Isolated workspaces can now be resumed into a new PTY, opened in a workspace-scoped diff, deleted, or force-deleted from the existing repo navigation surface.
- Added backend isolated-workspace deletion through `DELETE /api/workspaces/:id`, with live/orphaned session protection, dirty-worktree force checks, Git worktree removal, branch cleanup, and database state cleanup.

### Runner and integration tests

- Constrained runner-launched containers to the internal `sulion` Docker network and removed caller-controlled network selection from the Docker shim contract.
- Added `docker compose` and `docker-compose` shim support that maps Compose's default network to the external `sulion` network.
- Updated the backend integration harness to use the runner-exposed network path in Sulion PTYs while preserving native Docker host-port mapping outside the runner.

### Bug fixes

- Fixed timeline prompt submission so backend-injected prompts send Enter as a separate PTY input chunk instead of leaving text waiting at the agent prompt.
- Fixed isolated workspace deletion when the worktree directory is already missing but Git still has a registered worktree entry.
- Fixed the runner image so the Docker CLI and Compose plugin are installed and validated at image build time.
- Fixed the frontend nginx `/broker/*` proxy so browser secrets management routes are forwarded to the broker again while retaining Docker DNS re-resolution.
- Corrected the workspace dataset ownership contract so TrueNAS/ZFS bind mounts remain host-owned by `7321:7321` instead of relying on container-side ownership repair.

### Secrets and deployment

- Updated architecture, deployment, development, and toolset docs for workspace cleanup, the runner network boundary, Compose shim behavior, integration harness networking, and the secrets proxy fix.

## v1.1.0 - 2026-05-03

### Workspace isolation and agent flow

- Added Sulion-managed workspaces so PTY sessions can bind either to the canonical repo checkout or to an isolated Git worktree branch.
- Added workspace metadata on sessions, workspace-scoped file/diff/dirty/status APIs, and PTY environment variables plus `sulion workspace status` so agents can tell whether they are in `main` or an isolated worktree.
- Updated session creation so agent sessions default to isolated worktrees while still allowing explicit main-working-tree sessions.
- Added workspace-aware frontend routing for file tabs, diff tabs, file trace context menus, and session/sidebar indicators.
- Added first-class Claude/Codex launch support with `cl`/`co` executable shims, backend runtime metadata, and prompt injection from the timeline surface.

### Timeline and monitor UI

- Added the Monitor work-area tab for recent assistant output across active sessions.
- Added an input-only timeline prompt bar that can send prompts to a running agent without using the full terminal pane.
- Extended timeline/session metadata to surface agent runtime and transcript-reported model/context information.

### Container runner

- Replaced the old local Docker/Podman shim with a separate `runner` service that owns the host Docker socket and exposes a constrained command broker to PTYs.
- Added a PTY-visible `docker` wrapper that forwards allowed Docker commands to the runner with Sulion labels, resource defaults, and policy checks.
- Wired the runner to use the same canonical repo and isolated workspace mounts so `docker build .` works from either checkout.

### Runtime container and toolchain

- Rebased the backend/PTY image from Debian Trixie to Rocky Linux 10 to keep a glibc 2.39 runtime while using Rocky's `shadow-utils`/`newuidmap` behavior for nested rootless Podman without `SYS_ADMIN`.
- Translated the backend image package setup from `apt` to `dnf`, with EPEL/CRB enabled and Rocky package names for Podman, build tools, GitHub CLI, PostgreSQL client tooling, and shell utilities.
- Kept the existing PTY tool surface on the Rocky image, including Rust, .NET 8, .NET 10.0.100, Terraform, DuckDB CLI/Python binding, Node/pnpm, Python helpers, `uv`, `awscli2`, `git-lfs`, and the Sulion `docker` runner wrapper.
- Changed PTY `python3` to a Python 3.12 shim under `/usr/local/bin` while leaving Rocky's system Python path intact for `dnf`.

### Bug fixes

- Fixed backend startup so the API listener binds after migrations and orphan reconciliation, while derived transcript repair runs only when `ingest_projection_versions` is behind.
- Fixed transcript repair so it preserves source `events` rows and rebuilds derived canonical/timeline tables from existing Postgres payloads instead of deleting data and relying on JSONL replay.
- Fixed canonical-block repair so it skips already-populated events instead of reprocessing historical Codex events on every backend restart.
- Fixed a backend boot crash on deployed databases by restoring the original checksum for the already-applied canonical-block migration.

### Deployment and documentation

- Added the `runner` image/service to platform and compose wiring.
- Added a dedicated `/home/dev/workspaces` dataset/mount for Sulion-created worktrees.
- Updated agent-facing toolset docs, architecture docs, deployment docs, and state-management docs for workspaces, runner behavior, and the new PTY tool surface.

## v1.0.0 - 2026-05-02

### Security and secrets

- Added a separate Sulion secret broker service with encrypted-at-rest env bundle storage, isolated broker database usage, and a broker-only master key.
- Added per-PTY credential registration using signed secret-use requests, nonce replay protection, and revocation on PTY shutdown.
- Reworked credential consumption down to the two supported modes: `with-cred` for env injection and the Sulion `aws` wrapper for AWS CLI access.
- Changed secret reads so the UI receives metadata and env key names, not raw stored secret values. Existing values can be overwritten or preserved without being revealed.
- Added TTL-based per-terminal grants, active-grant revocation, conflict detection for overlapping `with-cred` env keys, and context-menu grant workflows.

### Timeline and ingestion

- Replaced full timeline polling with lightweight summary responses plus per-turn detail endpoints. The frontend now caches detail for older turns while refetching the active turn when its summary changes.
- Added repo/session turn-detail routes and repo-membership validation for repo-scoped timeline detail requests.
- Made timeline projection updates incremental for direct append cases instead of rebuilding the entire session projection every tick.
- Added batched dirty-transcript detection so ingest can stat files and load committed offsets in bulk before deciding what to read.
- Changed startup projection backfill to rebuild only sessions missing projection rows.

### Secrets UI and UX

- Added a dedicated Secrets work-area tab for creating and editing env-bundle secrets.
- Moved grant enablement out of the Secrets tab and into terminal/session right-click menus: Secrets -> Enable secret -> tool -> TTL.
- Added active-secret context-menu entries that show remaining TTL and revoke immediately when clicked.
- Added frontend state and tests for secret metadata, grant refresh, context-menu conflicts, and broker write responses.

### Runtime container and toolchain

- Added `sudo`, `git-lfs`, Terraform, DuckDB CLI, DuckDB Python bindings, `uv`/`uvx`, `click`, `Pillow`, and `rembg[cpu]` to the PTY image.
- Added .NET SDK support for both .NET 8 and .NET 10.0.100 so SDK resolution works for repos pinned to either SDK.
- Standardized Rust tooling in the image so `cargo`, `rustfmt`, and `cargo clippy` are available on the default shell `PATH`.
- Added `/opt/sulion/docs/toolset.md`, baked into the image outside workspace bind mounts, documenting the tools and Sulion-specific wrapper behavior available to agents.

### Bug fixes

- Fixed admin reindex so transcript replay preserves correlated terminal/session associations.

### Infrastructure and deployment

- Updated compose wiring for the broker registration token and per-PTY secret key flow.
- Updated Terraform outputs/platform registration and secret path registration for the broker integration.
- Added broker migration support for PTY credential registration and nonce tracking.

### Documentation and tests

- Added and refreshed docs for architecture, secrets, user workflow, deployment/tooling behavior, and current user-visible feature history.
- Added backend integration coverage for incremental projection, dirty-file filtering, reindex correlation preservation, and PTY association restoration.
- Added frontend unit coverage for timeline summary/detail behavior, secret context menus, and Secrets tab behavior.
- Added a real-stack Playwright secrets suite covering the supported secrets UX.
