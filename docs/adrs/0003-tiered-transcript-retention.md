# ADR 0003: Tiered Transcript Retention

## Status

Accepted, 2026-09-22.

## Context

Sulion stores every transcript line as JSONB in `events.payload` and derives
three further copies of the same text: canonical blocks, timeline operations,
and the rendered turn. On 2026-09-19 the `sulion` database was 30 GB with
about 1 GB of new payload a month; transcript-derived rows were 26 GB of it.

The database is also the only complete copy of Claude history. Claude Code
deletes its own transcript files after 30 days, and 745 sessions already
existed nowhere but in Postgres. Every rebuild path Sulion has reads
`events.payload`; none reads a file. Codex keeps its rollouts forever, so
5.2 GB of JSONL on the node was Codex and nobody was deleting it.

Retention therefore had to be decided for the database, and the question was
what a person or agent can still do with history older than the purge.

## Decision

History has two tiers.

**Live** sessions are stored as today. **Archived** sessions are those the
control process's archive loop has exported to an S3 bucket as one
compressed JSON-lines object of envelope records (offset, timestamp, kind,
tool-use link, payload) and, after a grace period, purged to a **turn
digest**: one row per turn holding the prompt, the rendered markdown, the
timestamps, tokens, and the files the turn touched, plus one embedding
source per turn. Cost is rolled up to `(day, repo, agent, model)` and file
churn to `(repo, path, day)` before the per-session rows go, with
per-session contribution tables so a restore subtracts exactly what was
added.

The archived contract: `sulion-retrieve search` finds turns by their
markdown and prompt; `turn` returns the same markdown; `file-history`
lists archived turns from the digest; cost and churn reports are the union
of rollups and live rows and do not change at the purge; the timeline pane
lists the turns and renders the markdown with an archived banner. Tool
output, thinking, operation-level search, and `--resume` need a restore.

A restore replays the archived lines through the same insert path a
transcript file takes, so the session comes back exactly as it would have
at first ingest; with `--purge-after` it is purged again once verified, so a
whole-history re-index never grows the database by more than one session.

The loop runs inside the control process on the backend's existing machine
identity. Objects are encrypted with SSE-S3 in a private, versioned,
TLS-only bucket that lifecycles to Glacier Instant Retrieval. The durable
tables (plans, sessions, settings, rollups, skeletons) are dumped with
`pg_dump` at the start of every cycle; transcript content is not dumped
because the session objects hold it.

## Alternatives considered

- **Archive the JSONL files and leave the database alone.** Rejected once
  measurement showed Claude's files are gone after 30 days and the database
  is the source every rebuild uses. Archiving files would have preserved
  only Codex history, and none of the database growth.
- **Keep every table's skeleton and null the large columns.** The first
  design. Rejected because it optimised for leaving queries unchanged
  rather than for what a user needs: search could find a text block whose
  turn had no readable body, and file touches, operation rows, and event
  rows survived without serving any archived use.
- **Roll cost up without a contribution table.** Simpler, but a restore
  could not take back what it had added, and a second purge after a restore
  would double count.
- **A separate archiver service with its own workload identity.** A
  narrower IAM role, at the cost of a fifth control-plane container, a
  workload declaration in `ahara-trust`, and a Terraform role, for three
  object-store calls. The backend's identity gains put, get, and list on one
  bucket; the boundary already allowed it.
- **`aws-sdk-s3` instead of the CLI.** The image already carries `awscli2`
  and the bootstrap sets the profile; the trust appliance's own backup uses
  the CLI. The object store is a small enum with a directory backend for
  tests, so the SDK bought nothing.
- **SSE-KMS with a project key.** The estate's precedent for secret
  material. Transcripts are code and prompts in a private bucket, and a
  project key needs the shared workload permissions boundary widened first.
  SSE-S3 now; the bucket policy and a boundary statement can move it later.
- **Delete file touches without a rollup.** File churn velocity is a metric
  the user asked to keep. The rollup and the digest's file list together
  cover the daily series and per-file history.
- **Keep `retrieval_embeddings.embedding REAL[]` beside the pgvector
  column.** First decided, then reversed. The array was the pre-pgvector
  store and the input of an exact-scan fallback that only the integration
  harness (stock `postgres:16`, no extension) ever exercised; every deployed
  topology uses the same TrueNAS Postgres with pgvector. Keeping it stored
  every vector twice (1.7 GB each) to preserve a test convenience. pgvector
  is now required: the migration owns the extension, column, and index, the
  harness and e2e stack run the `pgvector/pgvector:pg16` image, and the
  exact-scan path is gone.
- **Purge from the first cycle.** Rejected in favour of an operator gate.
  Deletion is off until `sulion archive purge-gate on`; the first cycle
  exports and dumps, `verify --deep` re-reads every object, and only then is
  the gate opened. The restore path exists, but the first deletion should
  not depend on it.

## Consequences

- Purged sessions are permanently smaller in the database and complete in
  S3. Losing the bucket loses the only copy of purged tool output; it is
  versioned, never expires current objects, and holds the durable dump.
- Every rebuild path must skip purged sessions. The guard is
  `claude_sessions.purged_at`; the projection writers and the admin reindex
  check it, and a new rebuild that forgets to would delete digests.
- The ingester cannot append to a purged session; it queues a restore and
  waits. A resumed Codex session older than the grace pays one replay.
- Archived search is turn-grained. A query that used to hit a tool result in
  old history now hits the turn that ran it, if the markdown mentions it.
- The cost report's repo attribution is frozen at purge time. Re-attributing
  a repo later changes live sessions only.
- The backend image carries the pgdg PostgreSQL 18 client for `pg_dump`.
