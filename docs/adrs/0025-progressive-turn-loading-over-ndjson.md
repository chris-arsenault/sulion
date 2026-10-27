# ADR 0025: Turn Detail Streams as Finite NDJSON over Authenticated HTTP with Keyset Pagination and a Shared Bounded Cache

## Status

Accepted, 2026-09-25.

## Context

After ADR 0024, turn detail was composed on read from `timeline_items` and
`timeline_operations`. The reader batched its SQL by turn but waited for
every item and every operation's full body before answering; the browser
client buffered the whole response text before parsing it; and
`TurnDetail.tsx` mounted every block. A long turn therefore showed nothing
until all of it had been read, serialised, transferred, and parsed.

The frontend compounded this. `TimelinePane.tsx` cleared its cache whenever
a display filter changed and restarted detail loads whenever a summary
object changed identity, so toggling "hide assistant text" refetched the
turn. The cache helper copied every array even for an empty delta, which
churned React identities. The subagent modal reloaded whole child
transcripts. These were source observations rather than measured deployed
latency, and the plan recorded them as such.

The boundary for the fix was fixed in advance: use the existing Postgres
projection and renderer, with no new ingestion path and no persistent
streaming service.

## Decision

Turn detail is a finite NDJSON response over the existing authenticated
HTTP route (`backend/src/api/turn_stream_routes.rs`,
`backend/src/ingest/projection/stream.rs`). The response carries a header,
then pages of up to 64 items or changed operations, then a completion
checkpoint. Each page's referenced operations, file touches, and child
totals are read in batches. The header arrives before any item read, so the
client can render the turn frame while rows are still being paged.

SQL reads use keyset pagination. A cursor binds the turn, the projection
generation, and the starting session offset; item and operation page
positions advance separately, and the completed synchronisation offset
advances only after both scans finish. Migration 0096 adds the generation,
which rebuilds rotate; purge state also participates, so a cursor from
before a rebuild or purge is refused rather than resumed. Reads may observe
operations newer than the cursor; operation versions stop an older metadata
or body response from overwriting a newer one, and the next delta starts
from the original offset, so repeating an update is safe. No database
transaction is held while waiting on the network consumer.

Operation headers are compact. Full input and result bodies load on demand
in batches of up to 16 when a tool is expanded or hovered, and a separate
read returns a complete digest for copying.

In the browser, `TurnDetailStore` holds unfiltered turn records, resumable
cursors, and in-flight reads keyed by transcript UUID and turn id, shared by
the session timeline, repository timeline, and subagent modal. Display
filters derive visible records locally and issue no requests. Revision
changes during a request coalesce into one later delta. Partial records
survive abort and retry, and errors leave visible content in place. The
store retains at most 24 turns unless an active consumer needs more.
Consecutive assistant records form groups of at most 64, which keeps group
identities stable across appends and lets text-only turns use the same
virtualised viewport as tool-heavy ones; detail views above 100 content
parts mount only nearby rows.

## Alternatives considered

- **Buffered whole-response reads.** The prior path, named in the plan's
  source evidence: `api/client.ts` buffered response text and
  `projection/view.rs` waited for all items and full bodies. Rejected
  because nothing rendered until everything had arrived. The fixture
  comparison in the plan showed the trade: an 8-item turn with a 1 MB tool
  result went from about 1 MB transferred before first paint to 2 kB, and a
  4,096-item turn's first batch arrived in about a fifth of the whole-read
  time, while total database work did not fall. Streaming buys earlier
  content and smaller initial transfer, not less work.
- **A persistent streaming service or WebSocket push.** Excluded by the
  plan's boundary ("no new ingestion path or persistent streaming
  service"). A finite response over the existing authenticated route reuses
  ALB JWT validation, the frontend proxy, and the client's auth handling;
  a push channel would have needed its own ticketing, reconnect, and
  revocation handling, as the terminal WebSocket already does.
- **Reloading on display filters.** The prior frontend behaviour.
  Rejected on inspection: a filter is a view over data the client already
  holds, and refetching it discarded cached turns and restarted in-flight
  loads. Milestone 2's acceptance was zero detail requests for a display
  filter change.
- **An unconditional pause between pages.** An early version yielded 16 ms
  per page to keep the main thread responsive. Replaced by an 8 ms
  processing budget followed by a task yield, after fixture measurements
  showed paging already adds round trips and the fixed pause only added
  latency.

## Consequences

- First content for a long turn arrives after one header read instead of
  after the whole turn; the client renders the frame, then fills rows.
- Total database work per full read is slightly higher than one buffered
  query because pages add round trips. That is accepted for earlier paint
  and bounded memory on both sides.
- Cursors are invalidated by rebuilds and purges. A client holding a stale
  cursor restarts the turn rather than merging across generations.
- Large tool bodies are not in the turn payload. Copy, export, and
  retrieval read them through their own paths; a feature that needs every
  body in the detail view must request them explicitly.
- Display filters, selection, scrolling, and expansion remain
  component-local; the store owns records and cursors only. Promoting
  interaction state into the store is a separate decision.
- Browser E2E and deployed LAN buffering were not verified at acceptance;
  the plan records loopback timings only. The HTTP integration test proves
  the header arrives while item reads are blocked.

## References

- Read contract and mechanism: [`../ingestion.md`](../ingestion.md), "Live
  projection"
- Cache ownership: [`../state-management.md`](../state-management.md),
  `TurnDetailStore`
- Published Sulion plan `6b29a9b0-f551-431c-804f-9b973f1c10cb`; its
  boundaries, source evidence, and fixture measurements are folded into
  this ADR
- Projection this reads:
  [`0024-incremental-per-event-timeline-projection.md`](0024-incremental-per-event-timeline-projection.md)
- Change record: `CHANGELOG.md`, Unreleased, "Turn details arrive in
  batches" and "Live updates coalesce into incremental reads"
- Commits: `9ee54db` — Stream and cache incremental turn details;
  `1f5e899` — Initialize streamed operations directly from branch
  expression
