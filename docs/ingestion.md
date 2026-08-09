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
