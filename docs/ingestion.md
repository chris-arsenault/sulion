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
reader that needs them opens timeline tool detail or queries operation rows.

## Usage accounting

Codex spend uses deduplicated `token_usage_record.payload.usage` responses,
including compaction. A session retains its legacy `token_count` prefix until
the first valid response record; subsequent counts update context pressure
without adding spend. Response IDs are persisted in `agent_usage_responses`
so duplicates at new byte offsets remain harmless after restarts. Missing
response IDs or usage objects do not switch the accounting source.

Claude assistant blocks can revise usage for the same message ID. The latest
receipt replaces the earlier contribution, including downward corrections and
changes to its day or model. Prior receipts remain in `events`; an indexed
lookup makes the correction survive restarts and interleaved responses.

Usage projection version 3 rebuilds retained sessions from stored events during
control startup, correcting older Claude totals and excluding inherited Codex
history. It preserves archived rollups. Daily projections and response
identities are replaced in the same transaction; the projection tables remain
locked until replacement completes. Admin reindex also rebuilds usage.

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
- The node process owns correlation and PTY management but does not poll JSONL;
  devenv owns the PTY masters and shadow emulators.
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

## Derived repair and background jobs

Startup maintenance versions canonical, timeline, and usage projections
independently; file touches belong to timeline projection. A version change
repairs the affected projection from stored events rather than forcing every
derivative to rebuild. Repair
failures in individual canonical rows are recorded and leave that version behind
for retry. Dependent usage and timeline repairs wait until canonical repair
succeeds.
`ingest_jobs` records job state, progress, and errors for the browser's jobs
panel; job completion is distinct from the background worker being alive.

Claude subagent transcript directories are discovered alongside primary logs.
Delegated logs and Codex child sessions use the same canonical model, with
concurrent child work matched to its owning operation. Bookkeeping records,
including per-response usage and Claude latch records, do not seed user turns.
Codex code-mode calls are unwrapped into operations so commands and native
patches remain visible in tool detail and file-touch evidence.

Queued Claude attachments become human turns only when their origin identifies
human input. Paired harness paste delimiters are removed from canonical text;
the pasted body and raw payload remain intact. Duplicate prompt UUIDs produce
one turn, and submitted prompts match against whitespace-normalized bodies.

Codex child sessions retain their owning metadata and use
`subagent_history_start_ordinal` to exclude copied parent history from turns,
usage, and activity. Agent messages retain their author, recipient, and available
text; encrypted content gets an explicit unavailable-content marker. New
`SubAgentActivity` start records connect spawning operations to child turns.
Completed runtime items enrich an existing operation by ID or a unique enclosing
code-mode execution, including yielded executions resumed by `wait`. Commands,
output, exit status, duration, and file changes remain available as operation
evidence. Ambiguous runtime records stay visible without adding an operation or
guessing their owner. Canonical version 5 and timeline version 11 repair retained
history through the same interpretation used by live ingest and archive restore.

## Retention boundary

`events.payload` is the full source record every rebuild reads. Source
transcript files disappear independently (Claude Code deletes its own after
30 days); ordinary startup repair and the admin reindex read the database
copy, never the file.

The archive loop in the control process (`backend/src/archive/`) is the one
thing that removes transcript rows, and only for a session whose export it
has verified in object storage. A session idle past `SULION_ARCHIVE_MIN_IDLE_DAYS`
is exported as one JSON-lines object of envelope records
(`{"o": byte_offset, "t": timestamp, "k": kind, "r": related_tool_use_id, "p": payload}`)
under `sessions/<agent>/<yyyy>/<mm>/<session>.jsonl.zst`; the offset is what
lets a restore land rows on the same `(session_uuid, byte_offset)` key the
file would. `SULION_ARCHIVE_PURGE_AFTER_DAYS` later the session is purged to
its **turn digest**: `timeline_turns` keeps preview, prompt text, markdown,
timestamps, tokens, and a `files_json` list of the paths each turn touched;
`claude_sessions`, `agent_session_metadata`, and `timeline_session_state`
stay; cost and file churn are rolled up into `usage_daily_rollup` and
`file_activity_daily` first (with per-session contribution tables so a
restore can subtract exactly what was added); everything else the session
owned — `events`, `event_blocks`, `timeline_operations`, touches, signals,
per-session usage, model switches, block- and operation-level embedding
sources — is deleted. `claude_sessions.purged_at` marks the state.

Three rules follow from the digest having no events behind it:

- `rebuild_session_projection`, its reconcile and incremental variants, and
  `rebuild_ingest_derivatives` skip purged sessions. A rebuild from zero
  events would delete the digest.
- The ingester refuses to append to a purged session. When a transcript file
  grows after its session was purged, `process_file` queues an
  `archive_requests` restore and leaves the offset alone; the next tick after
  the replay ingests the new lines normally.
- Restore replays the archived envelope lines through the same `insert_event`
  path a file takes (`ingest::replay_session_lines`), then rebuilds the
  projection in full. No transcript file is written and the ingester binary
  is unchanged.

Requests reach the loop through the `archive_requests` table: `sulion archive
run|restore|status|list` over the correlate socket, or `/api/admin/archive*`
from the UI. Progress is an `ingest_jobs` row. The durable tables are dumped
with `pg_dump` at the start of every cycle before anything is purged. Design
record: [`adrs/0003-tiered-transcript-retention.md`](adrs/0003-tiered-transcript-retention.md).
