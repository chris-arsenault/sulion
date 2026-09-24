# Incremental timeline writes using the existing model

Status: reset and replanned on 2026-09-24; implementation pending for a fresh agent.
This contract supersedes the previous generation-based replacement design.
Start with [the handoff](incremental-timeline-handoff.md).

Sulion plan: `3023d7e5-89ab-4c29-9506-032477ecbb85`.
The canceled predecessor is `e2029cd3-13a7-4e00-9de1-95adacfef341`; its completed
milestones describe the abandoned stash, not code present in this checkout.

## Outcome and scope

Appending a message or completing a tool must update only its timeline records
and small summaries. Cost must not grow with earlier operations in the turn or
untouched sibling transcripts. Long turns already work in the product; preserve
their presentation and interactions.

The user expects a few new tables, changes to existing persistence/API behavior,
and limited changes to UI data loading. Reuse the existing system. Do not restore
the stashed implementation wholesale or build another timeline subsystem.

## Evidence and reuse

The September 23 investigation of PTY `1b19da82` found about 90 seconds between
the last response and ingestion checkpoint, then 11.8 ms to timeline-state update.
Four child files preceded it, with approximately 12–14-second gaps. Each triggered
reconciliation of a shared parent with about 15,000 events and 3,927 operations.
The loop awaited that work and used file lengths captured before the scan.
These are historical observations, not a current deployment measurement.

The baseline code establishes the mechanism:

- `backend/src/ingest/projection/write.rs::rebuild_session_projection_after_insert`
  reloads the growing turn and falls back to whole-session reconciliation for
  descendants or evidence crossing the turn anchor.
- `backend/src/ingest/projection.rs::load_projection_source_events` loads and
  merges descendant transcripts.
- `backend/src/ingest/timeline/derived.rs` rederives every operation in that input.
  Conditional upserts reduce row churn but still visit the history.

| Existing responsibility | Reuse |
| --- | --- |
| Canonical evidence and block bodies | `events`, `event_blocks`, canonical adapters |
| Turn summaries and operation rows | `timeline_turns`, `timeline_operations` |
| File evidence and session revision | `timeline_file_touches`, `timeline_session_state` |
| Parsing progress and maintenance | `ingester_state`, current projection-version/reindex mechanism |
| Billing and revised response usage | Existing usage projection and receipt logic |
| Rendering and interaction state | `TurnDetail.tsx`, `SessionInspectorPane.tsx`, shared timeline controls |
| Retrieval, export, purge and restore | Existing consumers and their ownership contracts |

Read the current table migrations and consumers before adding replacements.

## Design boundary

1. Extend the existing rows with stable source identities and revisions where
   needed. Turn membership and tool/result lookup must survive restart and find
   the original owner after later prompts arrive.
2. Persist only missing state: processed source position/current stream ownership,
   independently addressable timeline items, and unresolved correlations or child
   references where existing records cannot carry them. Prefer references to
   canonical blocks over another copy of every input/result body. Map these
   responsibilities to a few tables or existing columns before coding; exact
   names and count are engineering decisions, not a quota.
3. Process a bounded batch in one transaction with its derived writes and cursor.
   Use a simple single-writer boundary per source. Keep projection work out of
   the serial file-reading path and bound per-file admission so busy sources
   cannot starve quiet sessions. Reuse existing process ownership.
4. Update only new/changed items, the affected operation, file evidence and count
   deltas. Correct prior usage contributions without adding another billable copy.
   Late evidence uses indexed ownership; it never triggers an ordinary turn replay.
5. Replace growing `chunks_json` writes with stored items, embedded
   `subagent_json` with child references, and live whole-turn markdown rewrites
   with composition for an explicit read/export/archive. Merely appending to a
   growing JSON/markdown value still rewrites history and does not solve the problem.
6. Adapt the current API and its loading/merge code to fetch items and corrections
   by stable identity. Retain the existing renderer, expansion/selection behavior,
   filters, thinking, file links, copy/library actions and child navigation.
   Do not introduce a second UI or a general body-fragment paging framework.
7. Have one live representation and writer after migration. Use the current
   maintenance/replay entry points for the transition and exceptional repair.
   Old archived digests remain readable because their source events were purged;
   this does not require keeping the old live projector operational.
8. Update retrieval only for changed content. Preserve source keys/deep links
   where possible, archive verification and restore accounting. Remove obsolete
   live reconciliation callers and duplicate structures when the replacement lands.

No new service, generic job framework, parallel active/building/rollback model,
second set of turn/operation tables, or independent runtime-repair scheduler is
part of this contract. If a concrete correctness requirement cannot fit this
boundary, show the failing case and propose the smallest adjustment before
expanding the architecture.

## Required behavior and decision ownership

Preserve Claude and Codex grouping; duplicate/inherited prompt handling;
queued human input versus notifications/bookkeeping; in-file sidechains and
external children; call/result pairing across turns; out-of-order and ambiguous
runtime evidence; revised usage; and source replacement detection. Never guess
an ambiguous owner or delete canonical evidence to make repair easier.

Only ingestion reads JSONL. API/WS readers query Postgres. Canonical insertion
and advertised progress must agree after crashes. Projection retries must not
double-count content, usage, file touches or children. Purged sessions stay outside
rebuilds until an authorized restore supplies their evidence.

Settled: existing-schema reuse, one live path, existing UI, affected-record writes.
Provisional: precise table/column map, batch budget and API cursor format. The
implementation agent resolves these from source inspection in M0, then records
the choices here. Migration locking/backfill and ID mapping must be explicit
before M2; avoid solving them by preserving every old operating mode.
Commit/push/deployment still require explicit authorization. This reset did not
authorize production database changes.

## Milestones

### M0 — Incremental writes on existing tables

Sulion phase: `21d14c9a-d506-4ffb-861f-d611c657d0d7`. Pending.

Write the small data/consumer map, then implement one complete new-event path
through existing persistence. Include late results, summary/usage contributions,
retry/restart behavior and fair ingestion/projection scheduling. Reuse focused
tests; do not construct all supporting subsystems before proving the write path.

Acceptance: appending a result visits its operation and related records only;
a cold restart resumes safely; older turns and sibling bodies are not loaded or
rewritten. Representative Claude and Codex corrections preserve ownership.

### M1 — Existing readers consume changed records [depends on M0]

Sulion phase: `e68c09de-9c57-4cb8-9767-91d320a3f299`. Pending.

Connect stored items and child references to existing API/UI consumers. Adjust
retrieval and explicit digest/archive reads where the live representation changes.

Acceptance: existing interactions work; open views merge new/changed records;
child updates do not download or rewrite the whole parent tree; file traces,
search and retained digests still resolve. No replacement renderer.

### M2 — Migrate and verify the complete path [depends on M1]

Sulion phase: `306e3047-da0b-4d2b-ba54-0496b399caad`. Pending.

Use one controlled migration/replay path without resetting source ingestion
checkpoints or rebuilding purged sessions. Remove obsolete live writers and
unneeded transition machinery. Review archive/restore and the final diff.

Acceptance: a fixed append does not scale with 10x historical turn size;
replaying 2x input takes approximately 2x work rather than 4x; quiet sessions
progress during concurrent child activity. Reuse the existing integration and
browser harnesses. Measure database work and source-to-ingest versus
ingest-to-visible latency separately; report actual limits, not synthetic
backend timings as proof of deployed/browser behavior.

The predecessor's p95 <2 s / p99 <5 s visibility numbers were provisional and
unverified. Measure the resulting behavior and report it; do not build extra
machinery solely to satisfy an unestablished latency promise.

## Restart state

All implementation from the rejected attempt, including its parser fix and tests,
is in stash `2e730845f0470300d7814cc8d8d320019e9dcd00`. No abandoned application
code or migration remains in the working tree. The separate handoff lists the
baseline, preserved unrelated edits, selective recovery candidates and test limits.
