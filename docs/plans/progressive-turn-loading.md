# Progressive turn loading

Sulion plan: `6b29a9b0-f551-431c-804f-9b973f1c10cb`.
Execution, commit and push authorized after upload change `61a34ae`.

## Outcome and boundaries

Show useful turn content before transfer completes. Preserve cached turns across
display filters and navigation, request deltas for updates, and bound rendering
and collapsed-tool payload work. Retain late results, child updates, archive
digests, scroll position, focus and manual expansion. Use the existing Postgres
projection and renderer; no new ingestion path or persistent streaming service.

## Source evidence

`TimelinePane.tsx` clears its cache on display filters and restarts detail loads
when summary objects change. `projection/view.rs` batches SQL by turn but waits
for all items and full operation bodies. `api/client.ts` buffers response text.
`turnDetailCache.ts` copies all arrays even for empty deltas. `TurnDetail.tsx`
mounts every block. `useSubagentStack.ts` reloads full child transcripts.
These are source observations, not measurements of deployed latency.

## Read contract

Finite NDJSON over authenticated HTTP carries a header, bounded batches and a
completion checkpoint. SQL reads use keyset pagination; each item batch loads
its operation references together. Operation metadata is compact; full bodies
are loaded only for expanded/hovered tools. Digest copying has a separate read.

A checkpoint names the projection generation and the starting session offset.
Item and operation page positions track partial progress separately. Advance the
completed synchronization offset only after both scans finish. Reads may observe
newer operations; their versions prevent older metadata or body responses from
overwriting newer ones. The next delta starts at the original offset, so these
updates are safe to repeat. Rebuilds rotate the generation; purge state also
participates in identity. Never keep a database transaction open for a slow
network consumer. Verify generation before accepting each batch and completion.

Display filtering operates on cached data. Selection filters remain in summary
queries. Cache and in-flight identity use session and turn, independent of summary
object identity. Revision changes during a request coalesce into a later delta.
Partial records survive abort and retry. Errors leave visible content in place.

## Milestones and acceptance

1. Confirm contract and baseline: inspect current sources, preserve existing
   tests, record request/payload baseline with representative fixtures. Live LAN
   measurements require an authorized accessible endpoint and are separate from
   local evidence.
2. Cache and refresh: zero detail requests for display filters, immediate cached
   navigation, one active request per turn, no empty-delta content churn.
3. Streaming: first batch arrives before completion; resume, concurrent writes,
   duplicate delivery and reset tests prove no lost records.
4. Payload/rendering: compact operation headers, on-demand bodies, stable grouped
   blocks, bounded mounted rows and preserved interaction state.
5. Related consumers: shared incremental subagent reads and complete digest-only
   copying, including archive behavior.
6. Verification: focused regressions, integration harness, frontend checks,
   repository gates and review; report actual performance and deployment limits.

## Execution expansion

M1: read current reader, cache, filter and projection-reset bodies; confirm the
protocol above against append/result ownership and archive behavior. Run existing
timeline frontend tests as baseline. Subsequent steps are expanded at execution.

M1 completed: baseline 46 tests pass across TimelinePane, TurnDetail and
turnDetailCache. Expansion `0abf1fc9-4a0c-43b1-84c9-c997f795807e` completed.
No deployed timing claim or LAN buffering proof has been established.

M2–M5 execution shares one API/cache boundary and one integration gate. Implement
the producer before wiring the consumers, retaining all four milestone criteria:

1. Add the generation migration and rotate it on projection reset. Implement
   bounded stream reads, cursor validation, compact operations and full-body and
   digest endpoints in the existing projection/API modules. Verify with real
   Postgres integration cases for batching, resume, late updates and reset.
2. Add the streaming parser and shared bounded cache; wire TimelinePane and
   subagent consumers with local display filtering and abort/coalescing. Verify
   fragmented wire records, interrupted loads and zero filter-triggered reads.
3. Preserve grouped block identities across appends and empty deltas. Virtualize
   long detail, hydrate only opened bodies, and retain focus/expansion behavior.
   Verify frontend regressions and type/lint checks. Shared checks gate completion.

M2–M5 implementation expansion `136d73e2-c9de-4254-8042-1a150b137e69` is complete.
The first two isolated integration runs passed 48 ingestion and 32 REST tests.
Frontend regressions cover filter request counts, coalescing, partial retries,
empty-delta identity, body version/generation ordering, eviction, nested subagents,
copy failure and virtualized mounting. The obsolete frontend whole-response merge
was removed after its consumers moved to the shared stream cache.

M6 execution:

1. Run final TypeScript, ESLint, frontend tests, Rust formatting, Clippy,
   structure/unit/doc tests, isolated REST/ingestion/archive integration tests and
   deployment configuration validation. Review the staged scope.
2. Record fixture payload and first-batch/total backend timings, without treating
   loopback measurements as deployed browser latency. Run the isolated Playwright
   suite if starting its temporary Vite server is authorized. A permission question
   is pending under the repository's explicit development-server rule.
3. Update the durable ingestion/state documentation and changelog, commit and push
   on the current branch as authorized. Record any browser and deployed LAN
   buffering checks that remain unverified.

Initial review corrected cursor turn binding, compact edited-file summaries,
virtual list sizing/spacing inside the existing flex scroller, and cache trimming
when a consumer leaves. A new copy-failure test initially targeted the prompt menu
instead of the turn menu; the corrected interaction passes.

Fixture measurements exposed a distinction between early content and total work:
paging adds database round trips. The final frontend review removed an unconditional
16 ms pause per page in favor of an 8 ms processing budget followed by a task yield.
Assistant-only runs now close groups after 64 records, preserving those groups on
append and allowing long text-only turns to use the same virtualized viewport.

## Verification evidence

The isolated Postgres fixture comparison uses one 1 MB tool result per turn.
One sample per fixture, original full read first; these are backend read and
serialization timings under concurrent compilation, not browser or LAN timings.
Stream bytes exclude the optional body request and include NDJSON checkpoints.

| Items | Full bytes | Stream bytes | Full read ms | First batch ms | Stream total ms |
|---|---:|---:|---:|---:|---:|
| 8 | 1,001,394 | 2,178 | 16.51 | 9.00 | 19.82 |
| 4,096 | 1,460,092 | 409,549 | 18.78 | 4.04 | 60.38 |

The original selected-turn reader already batched item/operation/touch SQL; the
verified regressions were repeated requests, discarded caches, full-body transfer
and whole-view rendering. Streaming trades additional bounded page queries for
earlier content and smaller initial transfer. It does not make total database
work faster in these fixtures.

Final backend gates passed: 304 library tests, binary/doc targets, two structure
checks, Clippy with warnings denied, formatting, and 87 isolated integration tests
(33 REST, 48 ingestion, six archive). Deployment configuration validation passed.
HTTP integration proves the header arrives while item reads are blocked. Frontend
tests prove progressive parsing, local filtering, cursor/version correctness,
bounded mounted rows and preserved interaction contracts. TypeScript and ESLint
pass; ESLint retains five warnings, including TimelinePane complexity 26/25.
The final full frontend run passed 392 tests across 61 files; an additional
text-only virtualization regression then passed with the existing viewport test.
Final verification/publication expansion: `d1b0529b-e850-48e4-96b5-f884a7cbf580`.

Browser E2E and deployed LAN buffering/latency remain unverified. The local
Playwright server-start permission question has not received an answer; no local
development server was started. The browser spec now checks that hiding/showing
assistant content issues no detail requests and remains available for that check.
