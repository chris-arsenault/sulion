# Incremental timeline projection for long agent sessions

Status: proposal, 2026-09-23. The user requested consideration and a design;
implementation and publication of code are not authorized by this request.
All implementation milestones are pending.

Sulion plan: `e2029cd3-13a7-4e00-9de1-95adacfef341`.

## Outcome and scope

A new message or tool result updates its own timeline entities and small
summaries. Its processing cost must not grow with the number of earlier
operations in the turn or the size of sibling subagent transcripts. Long,
continuously active turns and large agent trees are ordinary supported workloads.

Preserve canonical evidence, prompt grouping, tool correlation, late corrections,
usage attribution, nested agent navigation, file traces, retrieval, existing
deep links, archived digests, and restore. Keep both Claude and Codex on the
same projection model. REST and WebSocket readers continue to use Postgres;
only ingestion reads transcript files. No additional broker, database, or
service deployment is proposed.

## Evidence and the gap in the previous work

The earlier database change replaced destructive delete/reinsert projection with
conditional upserts. That reduces row-version churn, WAL and index maintenance
for unchanged entities. It does not eliminate loading, deriving, serializing,
comparing, or issuing SQL for those entities.

Current implementation has four sources of work proportional to history:

1. `ingest/projection/write.rs::rebuild_session_projection_after_insert` loads
   from the start of the affected turn; sessions with descendants reconcile the
   whole session. A late correction can also force whole-session reconciliation.
2. `ingest/projection.rs::load_projection_source_events` loads descendant
   transcripts, follows their lineage, and merges them with the parent.
3. `timeline_turns.chunks_json` and `markdown` grow with the turn;
   `timeline_operations.subagent_json` embeds child turns. A small child update
   changes a large parent value. The per-operation writer also revisits retrieval
   source identities even when the operation upsert changed nothing.
4. `TimelinePane.tsx` refetches entire selected-turn detail on invalidation;
   an open subagent modal follows the root resource revision. Fixing ingestion
   alone would leave growing network transfers and browser work.

Production diagnosis at 20:12–20:15 UTC on September 23 found a roughly
90-second delay for PTY `1b19da82`, followed by only 12 ms between its ingestion
checkpoint and timeline state update. Four Tsonu child files preceded it, with
roughly 12–14-second processing gaps. Their common parent projection held about
15,000 events, 58 turns and 3,927 operations. Ancestor reconciliation runs
inline after each child and bypasses that ancestor's debounce. File lengths
are captured before the pass, allowing messages to wait through two passes.
The deployed image was `a11b63e`; the later compatibility changes preserve this
underlying scheduling and reconciliation mechanism. No production benchmark
of the proposed replacement has been performed.

## Recommended architecture

### Durable ingestion and bounded projection are separate stages

Retain the ingester as the owner of source parsing and projection orchestration.
Run projection in a separate worker task with bounded concurrency and database
connections reserved for ingestion. Do not await projection in the file loop.
Bound ingestion work per file by records and bytes, so a transcript backlog
cannot monopolize ingestion either.

In a short transaction, ingest canonical records and advance a durable
per-session projection target to the last committed source position. The
checkpoint and target must never advertise records that did not commit.
An indexed target-versus-applied cursor can serve as the durable work queue;
an in-memory notification is only a wakeup optimization. Do not introduce a
global sequence-number watermark that could skip a transaction committed late.

A worker claims one session/generation using a lease with fencing, reads a
bounded committed range, loads only referenced state, and applies the changes.
Derived writes, summary deltas, dependency work, and the applied cursor commit
together. Each session has one projection writer; separate sessions can advance
independently. A target that advances during processing remains pending. A crash
before commit replays the range; a crash after commit resumes beyond it.

Use a fair queue with bounded batches and an explicit oldest-work policy.
Coalesce work for the same session and ancestor. Large replay jobs use a lower
work budget than live traffic. More workers cannot substitute for smaller work.
Surface failing records and projection lag instead of silently consuming a
cursor after a failed derived update.

### A turn is a small summary plus independently stored content

Proposed durable responsibilities; exact table names remain implementation detail:

| Record | Responsibility |
|---|---|
| Projection state | Generation, committed target, applied cursor, current local turn and lease |
| Event membership | Source event identity to owning turn; supports precise correction and replay |
| Turn summary | Stable ID, prompt reference/preview, timestamps, counts, flags and revisions |
| Timeline item | Ordered text/thinking/system fragment or reference to an operation |
| Operation | Stable source identity, call/result references, category and state |
| Dependency/link | Call-to-result, runtime-to-call, response-to-turn and delegation-to-child references |
| Derived contributions | File touches, usage and summary contributions attributable to one source entity |
| Projection changes | Revisioned entity upserts/removals for bounded client catch-up |

Reuse the existing operations, file-touch, usage and retrieval tables where
their ownership and keys fit. Replace position-based operation identity with a
stable key derived from the originating session, call event offset and block
ordinal; harness call IDs are indexed correlation keys, not assumed globally
unique. Ordinals are ordering metadata, never identities whose insertion would
renumber all later operations or retrieval sources. Preserve current public
turn identifiers and explicit aliases for historical merged child turns.

Persist content as source-addressable, byte-bounded fragments. Append new items;
replace only the affected fragment for a revised message. A huge tool result
must not become the next ever-growing hot row: store its content separately and
page it, retaining exact source references. Keep unresolved calls and other
dependencies as indexed records, not an ever-growing JSON checkpoint.

Project events through one deterministic incremental reducer shared by live
processing and replay. Its inputs are the newly committed records and indexed
dependencies. Its output is an explicit set of entity changes. It never
reconstructs a full turn to discover what changed, and never deletes all rows
absent from a session-wide expected-key list during ordinary live processing.

Maintain summaries using old/new contribution differences. For example, an
operation completion decrements pending count, updates error count and badges,
and adjusts its timestamps. Counts backing booleans allow corrections to remove
errors correctly. Separate a session's own usage from descendant usage so
linking a child never creates a second billable copy. Exceptional deletion or
timestamp correction may use indexed per-entity/per-turn aggregate repair;
this is explicit repair work, not the append path.

### Child sessions own their data

Each child has one projection of its own turns and operations. The parent
delegation stores the child-session reference and a small status/summary, never
the child's transcript JSON. Distinguish delegation edges from continuation
edges; the existing nullable parent relationship alone cannot encode both
meanings reliably.

A child update changes child entities and its compact summary. Parent links
receive coalesced revision/summary changes. If an inclusive ancestor summary is
needed, propagate a versioned old/new child contribution once per edge. Work
scales with actual ancestor depth, not all siblings or all descendants. Reject
cycles and duplicate edges; unresolved linkage can be attached later without
copying or replaying the child content. In-file Claude sidechains need explicit
logical stream ownership even when they do not have a separate source file.

### Corrections are dependencies, not reasons to replay a turn

| Incoming evidence | Affected work |
|---|---|
| Ordinary assistant text | New fragment(s), owning turn/session summary |
| Tool result or error revision | Referenced operation/result, its touches and contribution deltas |
| Revised Claude response usage | Response contribution and original owning turn/day/model totals |
| Codex completed runtime item | Indexed matching call or durable unresolved evidence |
| Child message | Child projection; compact parent-link notification |
| Late delegation link | Link and summary contribution, without child transcript copy |
| Replayed input | No change after identity/cursor deduplication |
| Parser/rule version change | Explicit generation rebuild using the same reducer |

Out-of-order call/result and parent/child arrival must converge. Ambiguous
runtime evidence remains unresolved and visible; never guess its owner to
avoid work. Source-session byte offsets provide local processing order;
cross-session timestamps are display ordering, not transaction causality.
Task notifications, queued human prompts, inherited Codex history, and compaction
keep their established semantic distinctions.

A turn ending does not make its operations immutable: background results and
usage corrections may arrive later. Index them back to their original owners.
Do not rely on closing or artificially splitting long turns to obtain bounded
work. Source replacement/truncation is an explicit repair boundary with a new
generation, not permission to overwrite immutable raw evidence.

### Readers and derived consumers follow the same granularity

Timeline summaries remain small and paginated. Turn detail becomes a page of
items and operation references. A scoped revision/change cursor returns only
new or changed entities, including removals and corrections to earlier pages.
Opening a child follows its reference and subscribes to that child's revision.
Closed siblings require no detail fetch. Virtualize content inside long turns,
not only the list of turns.

Each response identifies its committed projection revision. The client merges
changes by stable ID, retains scroll/selection, and ignores superseded responses.
If a retained change cursor expires, refetch the visible pages at a consistent
revision; do not fall back to downloading the entire tree. Filters, repo views,
deep links and pending/error badges must retain their current meaning. Global
session revision can remain a small invalidation hint, not an instruction to
reload every descendant.

Live ingestion no longer regenerates whole-turn markdown. `sulion-retrieve turn`
and explicit exports compose ordered fragments when requested; full export is
naturally proportional to requested content and should support bounded/streamed
output. Existing archived turn digests stay readable. Retained-history search
uses fragment/operation indexes; retrieval source invalidation happens only for
entities whose indexed text changed. Audit both lexical and semantic readers
before retiring the old materialized markdown path.

At archive time, finish projection through the verified export watermark,
assemble the retained digest and file summary, and then use the existing purge
protocol. A continuing child prevents freezing an inconsistent linked view.
Define ownership of parent/child archival summaries explicitly so file and
usage rollups count each source once. Restore replays the same reducer in
bounded batches. Purged sessions with no raw events remain outside rebuilds
until an authorized restore supplies them.

## Cost contract and alternatives

Ordinary event processing should cost the new input bytes plus the affected
entities and indexed dependency lookups. Tree summary propagation may cost
ancestor depth. It must not visit historical operations or untouched sibling
content. Replaying N append events should require approximately linear total
work, allowing index factors, rather than summing repeated prefixes to N squared.

Conditional upserts remain useful as a final guard. Longer debounce intervals,
more concurrent full rebuilds, moving the current projector into a queue, or
rebuilding only the current turn do not satisfy the cost contract. Fixed-size
content fragments help only if reducers also stop reconstructing the complete
turn. Keep a full semantic comparison path during migration, not a permanent
second live projection architecture.

This design adds durable identity/dependency state and a paginated client
protocol. That is the principal complexity trade-off. It is justified because
the current nested mutable values require history-proportional work regardless
of scheduling or database tuning.

## Milestones and acceptance

### M0 — Contract and workload baseline

Scope: enumerate grouping/correction semantics, existing ID consumers, in-file
sidechains, archive ownership, and representative workload measurements.
Acceptance: agreed invariants and a reproducible baseline covering small and
very long turns, 1/8/32 concurrent children, nested delegation, and quiet sessions
receiving new input during heavy activity. Count SQL round trips, entities and
bytes read/written, CPU, WAL, queue age, response bytes and frontend updates.
Record measured capacity; do not imply unlimited children on fixed hardware.
Evidence: source-contract review and sanitized/generated replay fixtures.
Sulion phase: `bc2c53a7-6cb7-40d7-b8ab-e151e25e5a60`.

### M1 — Durable incremental state [depends on M0]

Scope: stable keys, membership/dependency indexes, projection generation/cursors,
durable scheduling and fenced single-session ownership.
Acceptance: atomic ingest-to-target and reducer-to-cursor boundaries survive
crashes, retries, lost wakeups, lease replacement and overlapping repair requests.
No live API cutover yet. Evidence: isolated database fault/restart tests.
Sulion phase: `f605beb9-e762-4a51-9d13-60549e584ec6`.

### M2 — Incremental reducers and scheduling [depends on M1]

Scope: canonical event reducers, bounded fragments, operation/usage/touch deltas,
agent links, fair work budgets and independent ingestion progress.
Acceptance: a single appended result touches no unrelated operation or sibling
body; cold restart loads referenced state, not the complete open turn. Corrections
and out-of-order relationships converge without whole-turn live reconciliation.
Evidence: live-versus-replay parity and instrumented entity/byte work counts.
Sulion phase: `0205394c-6cdf-49a5-88e7-1cd405d37e99`.

### M3 — Readers, retrieval and archive [depends on M2]

Scope: paged detail and scoped changes, child navigation, frontend merging,
search/source keys, on-demand digest composition, archive and restore contracts.
Acceptance: current user-visible semantics and old links survive; a new child
message neither retransmits the root tree nor rewrites its digest. Open long
turns remain bounded in response/render size. Verified archive/restore produces
equivalent usage, file traces and readable digests without duplicating ownership.
Evidence: targeted browser/API, retrieval and archive round-trip checks.
Sulion phase: `c34a50a0-7054-4cb1-b533-03430a6a8e9d`.

### M4 — Replay, cutover and performance proof [depends on M3]

Scope: shadow a new generation from retained events with the same reducer;
throttle catch-up, compare semantics, and switch complete sessions atomically.
Do not reset source ingestion offsets. Maintain an explicit rollback generation
and a compatibility path for already archived digests; remove old full-reconcile
live writers and nested payload columns after the rollback window.

Acceptance: with a fixed new-event batch, increasing historical turn size tenfold
does not cause proportional entity reads, writes or SQL calls. Doubling total
replay input produces approximately double work, not four times. A busy agent
tree cannot monopolize service for another active session below measured capacity.
Provisional live visibility target: p95 under two seconds and p99 under five
seconds at the measured supported load, including browser polling; establish
this baseline in M0 and report any required change rather than weakening it
silently. Track source-to-ingest and ingest-to-visible latency separately.
Evidence: scaling matrix, correctness comparison, crash recovery, then an
authorized deployment with telemetry and rollback criteria.
Sulion phase: `3ae685e5-5ed8-44e5-94ab-a4f3894a34e2`.

## Decision status and handoff

Settled constraints: preserve raw evidence and existing semantics; avoid
history-proportional live reconciliation; account for concurrent agent trees;
respect ingestion, archive and credential ownership boundaries.

Recommended, not yet accepted implementation: the incremental durable model
above, PostgreSQL work cursors, one projection per child, and paged consumers.
Exact batch byte budgets, worker concurrency, fragment size and change-retention
window are provisional engineering choices to measure in M0/M2/M3. The
implementation owner resolves them against the cost and correctness contracts.
Archive-link retention details and ID alias migration must be settled by M0
before M1 schema design. Deployment/cutover timing is the user's choice at M4.

Completed here: repository and consumer review, production diagnosis, proposed
contract and milestone publication. No implementation, migrations, tests or
production changes were performed for this proposal. Next action, if execution
is authorized: expand M0 with `plan-phase`; keep the remaining milestones pending.
