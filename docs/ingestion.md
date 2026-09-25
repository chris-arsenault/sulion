# Ingestion runtime boundary

The dedicated development-node role runs one separate `sulion-ingester`
process. The portable loopback standalone role retains in-process ingestion as
a rollback seam, but remote-node control never mounts or reads transcript
files.

## Current boundary

Code and process ownership are split:

- `backend/src/ingest/canonical/` — source-specific transcript translation
- `backend/src/ingest/timeline/` — event interpretation and the incremental timeline reducer
- `backend/src/ingest/projection.rs` — the projection batch writer and `timeline_*` readers
- `backend/src/ingest/ingester.rs` — polling + orchestration

Claude and Codex write into the same canonical event model, share
`ingester_state`, event storage, block storage, and projection behavior. The
one ingester receives both read-only transcript roots. Splitting by agent
family would duplicate ownership without buying failure isolation.

## Live projection

Reading and projecting run side by side. Every 500 ms the ingester reads each
transcript that grew, inserts at most 2,000 lines per file
(`ADMIT_LINES_PER_TICK`), and queues the sessions it inserted into. A
projection worker in the same process serves that queue one 500-event batch
at a time, round robin, so a long backlog never holds a quiet session behind
it. On start the worker queues every session whose events run past its cursor.

A batch locks the session's `timeline_session_state` row, feeds the events
after `projected_through` to the reducer (`backend/src/ingest/timeline/reduce.rs`),
and commits what they changed together with the new cursor. An appended event
writes its turn's counters, at most one `timeline_items` row keyed by its byte
offset, and the operation its call, result or runtime evidence belongs to,
with that operation's file touches. Items are never rewritten. An operation
update records the event's offset in `changed_at`. A Claude usage receipt
updates the row in `timeline_message_usage` for its message and charges the
difference to the turn that first reported it; Codex turns charge running
totals against a per-turn baseline. Earlier turns and sibling transcripts are
not read. A result finds its call by pair id in any turn; runtime evidence
finds its call by id, or else the single `exec` whose run encloses it, and
evidence that matches none or several stays on its own event as bookkeeping. A
result with no earlier call is not paired. Bookkeeping before a session's
first prompt is not projected. Events inserted behind the cursor, as a
replaced transcript does, mark the session for rebuild: its projection is
emptied and replayed through the same reducer. The row lock makes each batch
the single writer, whichever process runs it.

Turn detail is served from `timeline_items` and `timeline_operations`. The
response carries `through`, the session's cursor when the read began; an open
view reads a turn once and then asks for `since=<through>`, which returns the
items at later offsets and the operations with a later `changed_at`. The
client joins consecutive assistant items into one block and lists a block's
calls after it. A spawning call references its child transcript through
`timeline_child_links`; a Claude subagent's batch inserts that link, and
nothing else, into its parent. Every read attaches the child's current totals,
and the app-state timeline revision of a session adds its direct children's
revisions, so an open parent view refreshes as a child grows.
`markdown` is the turn digest that `sulion-retrieve turn` returns: the prompt,
the assistant's text blocks, and one header line per tool call. Live turns
compose it on read; the archive purge stores it. Tool inputs, diffs, and
result bodies are not part of it; a reader that needs them opens timeline tool
detail or queries operation rows.

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
`SubAgentActivity` start records link spawning operations to the child session.
Completed runtime items enrich an existing operation by ID or a unique enclosing
code-mode execution, including yielded executions resumed by `wait`. Commands,
output, exit status, duration, and file changes remain available as operation
evidence. Ambiguous runtime records stay visible without adding an operation or
guessing their owner. Canonical version 5 and timeline version 12 repair retained
history through the same reducer used by live ingest and archive restore; the
timeline repair rebuilds the most recently active sessions first.

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
owned — `events`, `event_blocks`, `timeline_operations`, items, touches,
per-session usage, model switches, block- and operation-level embedding
sources — is deleted. `claude_sessions.purged_at` marks the state.

Three rules follow from the digest having no events behind it:

- The projection batch, `rebuild_session_projection`, and
  `rebuild_ingest_derivatives` skip purged sessions. A rebuild from zero
  events would delete the digest.
- The ingester refuses to append to a purged session. When a transcript file
  grows after its session was purged, `process_file` queues an
  `archive_requests` restore and leaves the offset alone; the next tick after
  the replay ingests the new lines normally.
- A purge freezes the session's usage and digest as the current projections
  hold them, so `purge_session` refuses while any startup repair
  (`canonical_blocks`, `usage_projection`, `timeline_projection` in
  `ingest_projection_versions`) is behind the binary, or while the session's
  timeline cursor lags its events. The archive cycle checks the repairs once
  and defers the whole purge phase to the next cycle.
- Restore replays the archived envelope lines through the same `insert_event`
  path a file takes (`ingest::replay_session_lines`), then rebuilds the
  projection in full. No transcript file is written and the ingester binary
  is unchanged.

Requests reach the loop through the `archive_requests` table: `sulion archive
run|restore|status|list` over the correlate socket, or `/api/admin/archive*`
from the UI. Progress is an `ingest_jobs` row. The durable tables are dumped
with `pg_dump` at the start of every cycle before anything is purged. Design
record: [`adrs/0003-tiered-transcript-retention.md`](adrs/0003-tiered-transcript-retention.md).
