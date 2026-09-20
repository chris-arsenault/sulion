# Ingestion runtime boundary

The dedicated development-node role runs one separate `sulion-ingester`
process. The portable loopback standalone role retains in-process ingestion as
a rollback seam, but remote-node control never mounts or reads transcript
files.

## Current boundary

Code and process ownership are split:

- `backend/src/ingest/canonical/` — source-specific transcript translation
- `backend/src/ingest/timeline/` — app-shaped timeline formation and projection derivation
- `backend/src/ingest/projection.rs` — materialization into `timeline_*` tables
- `backend/src/ingest/ingester.rs` — polling + orchestration

Claude and Codex write into the same canonical event model, share
`ingester_state`, event storage, block storage, and projection behavior. The
one ingester receives both read-only transcript roots. Splitting by agent
family would duplicate ownership without buying failure isolation.

## Live projection cadence

The ingester polls transcripts every 500 ms and re-derives a session's
timeline projection from the turn that grew. A live turn is re-upserted
whole, and a long turn's markdown and chunks run to megabytes, so projecting
on every tick would rewrite that payload once per appended event. While a
session keeps growing it is therefore projected at most once every 5 seconds
(`LIVE_PROJECTION_DEBOUNCE`); the first tick after it stops growing projects
it regardless, so a finished turn shows within a tick. The debounce is a
config value that defaults to zero, which the tests rely on; the ingester
and standalone binaries pass the 5-second interval.

`timeline_turns` stores no whole-turn JSON. Turn detail is served from
`chunks_json` and `timeline_operations`; every rebuild derives from
`events.payload`. `markdown` is the turn digest that `sulion-retrieve turn`
returns: the prompt, the assistant's text blocks, and one header line per
tool call. Tool inputs, diffs, and result bodies are not part of it; an
agent that needs them reads the operation rows or restores the session.

## Usage accounting

Codex spend uses deduplicated `token_usage_record.payload.usage` responses,
including compaction. A session retains its legacy `token_count` prefix until
the first valid response record; subsequent counts update context pressure
without adding spend. Response IDs are persisted in `agent_usage_responses`
so duplicates at new byte offsets remain harmless after restarts. Missing
response IDs or usage objects do not switch the accounting source.

Usage projection version 2 repairs response-record sessions from stored events
during control startup. Upgrades preserve existing legacy-only and Claude
projections; databases without version 1 rebuild those first. Repair replays
model changes in event
order for Codex response sessions, and replaces daily projections and response
identities in the same transaction. The projection tables remain locked until
the replacement is complete, so concurrent ingestion continues afterward.

Metrics apply the catalog rates dated in the response to all historical dates;
they estimate API-equivalent usage rather than historical invoices. Unpriced
usage is marked as incomplete in totals and on the chart. Repository hash
fallbacks use a mapping computed once per query to avoid repeated scans of the
frequently updated runtime-state table.

## Process ownership

- Control owns SQLx migrations and Postgres-only startup maintenance.
- `sulion-ingester` waits for the control-owned migration set, then owns
  transcript polling and new-line projection.
- Derived repair remains gated by `ingest_projection_versions`. Control repairs
  missing canonical/timeline fields from existing `events.payload` rows and
  never replays JSONL during ordinary startup.
- The node process owns correlation and PTYs but does not poll JSONL.
- API handlers and WebSocket paths query Postgres only.

The ingester restarts independently. A control or network outage leaves local
append-only transcripts intact; after Postgres recovers, the worker resumes
from the last committed byte offset.

## Binary and deployment

`backend/src/bin/sulion_ingester.rs` is mapped into the shared workbench image.
The dedicated Compose role invokes it directly and mounts only
`~/.claude/projects` and `~/.codex/sessions`. API readiness depends on
Postgres/migrations, not ingester liveness; ingester failures are visible in
its independent service logs. Failure and compatibility semantics are in
[`node-protocol.md`](node-protocol.md).
