# ADR 0024: The Timeline Is Projected Incrementally from Per-Event Items with a Single Reducer

## Status

Accepted, 2026-09-24.

## Context

The timeline projector rebuilt whole turns. When a transcript grew, the
write path reloaded the growing turn, rederived every operation in it, and
fell back to whole-session reconciliation whenever a descendant transcript
or late evidence crossed the turn anchor. Child files triggered the same
reconciliation on their parent.

On 2026-09-23 an investigation of one PTY measured about 90 seconds between
an assistant's final response and its ingestion checkpoint, followed by
11.8 ms to the timeline-state update. Four child files had been processed
first, each reconciling the same parent of roughly 15,000 events and 3,927
operations, and the serial ingestion loop awaited each projection. The cost
of appending one event grew with the history of the turn and with sibling
transcripts that had not changed.

The stored shape made this structural. `timeline_turns.chunks_json` held
every visible event of a turn as one growing JSON value, `subagent_json`
embedded merged descendant turns, and the digest markdown was rewritten per
event (ADR 0023 reduced its size, not its rewrite). Appending to any of
these rewrote the row.

A first replacement attempt built a generation-based active/building/
rollback projection with its own tables, queue, compatibility layer, and
renderer: 81 files, 7,598 added lines. The user rejected it as
overcomplicated on 2026-09-24. It was stashed as `2e730845` for reference,
the working tree reset to baseline `7b799ef`, and a new contract written
with the instruction to reuse the existing model and build no second
timeline subsystem.

## Decision

One reducer (`backend/src/ingest/timeline/reduce.rs`) consumes a session's
events in byte-offset order. Each event appends at most one
`timeline_items(session_uuid, turn_id, byte_offset, body)` row and inserts
or updates only the operation it belongs to, with that operation's file
touches and the turn's counters. Items are never rewritten. An operation
update records the event's offset in `changed_at`. Grouping, digests, and
child totals are derived on read.

The batch is the unit of work and of locking: 500 events per transaction,
with the session's `timeline_session_state` row locked `FOR UPDATE`, so the
reducer is the single writer across the ingester and maintenance processes.
The cursor (`projected_through`) commits with the rows it produced. A
projection worker in the ingester serves queued sessions round robin, one
batch at a time, so a long backlog does not hold a quiet session behind it;
file admission is bounded per tick for the same reason.

Missing state lands on the existing tables: open main and sidechain turn,
Codex running totals, and reducer version on `timeline_session_state`;
prompt dedupe and Codex usage baselines on `timeline_turns`; call/result
lookup and runtime-evidence correlation on `timeline_operations`; revised
Claude usage in `timeline_message_usage`; and child references in
`timeline_child_links`, which replace embedded descendant turns with a link
from the spawning call to the child's own transcript.

Readers group items, compose digests, and attach child references on read.
Turn detail returns `through`, the cursor at read time; an open view reads
a turn once and then asks for `since=<through>`, which returns later items
and operations with a later `changed_at`. An item's visibility never
changes under the filters, so a delta needs no removal list. A session's
app-state revision adds its direct children's revisions, so an open parent
refreshes while no parent row changes.

Migration 0095 drops `chunks_json`, `subagent_json`, and
`timeline_activity_signals`. Projection version 12 replays every retained
session through the reducer from its canonical events, most recently active
first; purged sessions stay outside it. The whole-session pass is deleted,
and the same reducer serves live ingest, rebuild, and archive restore.

Some behaviours differ from the whole-session pass by intent: events apply
in byte-offset order rather than timestamp order; a result pairs with the
latest earlier call carrying its id in any turn; bookkeeping before the
first prompt is not projected; runtime evidence is placed with what has
arrived; a child's transcript is referenced, not merged into its parent's
event count.

## Alternatives considered

- **Whole-turn rebuild with parent reconciliation.** The prior mechanism.
  Rejected on the 2026-09-23 measurement recorded in the plan: about 90 s
  from response to checkpoint, driven by repeated reconciliation of a
  15,000-event parent for each child append. Conditional upserts had reduced
  row churn but still visited the history on every event.
- **Queue or coalesce whole projections.** The original recommendation of
  the delay investigation. Superseded in the handoff, which states that
  ordinary updates must stop rebuilding history rather than schedule the
  rebuild differently; coalescing changes when the 90 s is paid, not
  whether.
- **Append to the growing JSON or markdown values.** Rejected in the
  contract's design boundary: appending to a growing value still rewrites
  the whole row, so the per-event cost stays proportional to the turn.
- **Generation-based active/building/rollback projection.** The stashed
  first attempt (`2e730845`): parallel table sets, a projection queue,
  compatibility modes, and a replacement renderer. Rejected by the user as
  overcomplicated. The replacement contract names what was over-built: no
  new service, generic job framework, parallel active/building/rollback
  model, second set of turn/operation tables, or independent repair
  scheduler. Its useful behavioural test cases and one parser fix were
  recovered selectively through `git show`; nothing was applied wholesale.

## Consequences

- Appending a call and its result after 2,000 earlier calls in the same
  turn rewrites one existing row (the turn) and adds two. Rebuild cost is
  linear in events.
- There is one live representation and one writer. Rebuild, restore, and
  live ingest cannot drift because they are the same code path.
- Turn ids, operation ordinals, and retrieval source keys keep their
  formulas, so deep links and embedding source keys survived the
  migration.
- The read side does more: grouping and digest composition happen per
  request. ADR 0025 bounds that with paged, streamed reads.
- A result now pairs across turns. The result's own turn still records its
  error, but tooling that assumed pairing within a turn had to change.
- Sessions projected before version 12 served an empty detail until their
  rebuild ran; a live session rebuilt on its next batch.
- The plan's `/tmp` evidence file and the stash are not durable. The
  measurements that justified the change are the ones recorded here and in
  the plan's history; the stash exists only as a reference for what not to
  rebuild.

## References

- Live projection mechanism: [`../ingestion.md`](../ingestion.md), "Live
  projection"
- Published Sulion plan `3023d7e5-89ab-4c29-9506-032477ecbb85` (cancelled
  predecessor `e2029cd3-13a7-4e00-9de1-95adacfef341`); the working
  contract, data map, reader decisions, and reset record that accompanied
  it are folded into this ADR (stash
  `2e730845f0470300d7814cc8d8d320019e9dcd00`, baseline `7b799ef`)
- Digest contents:
  [`0023-turn-digest-is-prompt-text-and-tool-headers.md`](0023-turn-digest-is-prompt-text-and-tool-headers.md)
- Streamed reads over these rows:
  [`0025-progressive-turn-loading-over-ndjson.md`](0025-progressive-turn-loading-over-ndjson.md)
- Commit: `e02492f` — Project the timeline incrementally from per-event
  items
