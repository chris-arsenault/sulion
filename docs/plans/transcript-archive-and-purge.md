# Database archive, backup, and monthly slimming

Plan only. No code, schema, infrastructure, or database change has been made.
Status: proposal awaiting the decisions marked `[DECISION]`.

Revision 2 (2026-09-19): scope moved from JSONL files to the database after
measurement showed the database is already the only complete copy of
transcript history. See "Why the database, not the files".

## Outcome and scope

The `sulion` database on TrueNAS is 30 GB and grows by roughly 1 GB of
transcript payload per month, four times over across projections. This plan
adds a control-plane archive cycle that:

1. **Exports each idle agent session's transcript to S3** as one JSON-lines
   object reconstructed from `events.payload`, under the backend's existing
   machine identity from the trust appliance.
2. **Dumps the durable, non-derivable tables** to S3 with `pg_dump` on the
   same monthly cycle, so plans, sessions, usage aggregates, and settings
   survive losing the instance.
3. **Slims transcript-derived bulk** for sessions whose export is verified
   and whose grace period has passed: raw payload, tool bodies, turn
   renderings, and tool-call embeddings go; session skeletons, daily usage,
   file touches, natural-language text, and plans stay, so cost reporting,
   churn velocity, plan flow, and most of `sulion-retrieve` keep working.
4. **Restores any session from S3 on request**, replaying its lines through
   the normal ingest path into the database, and re-slims it afterwards.
   A whole-history re-index is the same operation batched by month.

Non-goals: JSONL files on the node (Claude Code's own 30-day cleanup stays
as is; Codex rollouts are left alone), Codex's own state under `~/.codex`,
the code-intelligence tables, and public edge work.

Authorized so far: this plan document and its Sulion plan. Execution needs
separate authorization per milestone; M1 touches this repository's Terraform
and the shared `ahara-trust` site policy only if a separate identity is
chosen (decision 4).

## Why the database, not the files

Measured 2026-09-19 (details in "Measured state"):

- Claude Code deletes its own transcripts after 30 days
  (`~/.claude/.last-cleanup`, no `cleanupPeriodDays` override). 745 Claude
  sessions exist only in the database today; every Claude session older than
  30 days will, forever, unless that setting changes.
- Every rebuild path Sulion has, including `/api/admin/reindex`, the
  projection-version startup repairs, and the retrieval reset, reads
  `events.payload`. None reads JSONL. No code path deletes `events`,
  `claude_sessions`, or `ingester_state` rows. Vacuumed sessions therefore
  survive every reindex.
- The database is 30 GB while all JSONL on disk is 5.6 GB, of which 5.2 GB
  is Codex that nobody is deleting.

So `events.payload` is the archive source, S3 is where it goes, and the
files on the node are Claude's and Codex's business. This removes the
node-side archiver, the read-write transcript mounts, the new workload
identity on the enclave, and the mtime problem from the previous revision.

## Measured state

Disk on the dedicated node (devenv PTY, `du`, `find`):

| Root | Bytes | Files |
| --- | --- | --- |
| `~/.claude/projects` | 384 MB | 233 `.jsonl` (119 subagent files) |
| `~/.codex/sessions` | 5.2 GB | 1465 rollouts, April to September |

`ingester_state` (1912 rows) against disk: 1689 files present and fully
ingested, 0 shorter or longer than their offset, 223 missing (all Claude,
645 MB committed, last written 2026-07-28 to 2026-08-21). A further 552
Claude sessions (last events April to June, `/home/dev` era) have events but
no `ingester_state` row and no file. Total database-only: **745 Claude
sessions, 333,312 events, about 820 MB of compressed payload.**

Database, PostgreSQL 18.4, 30 GB:

| Table | Total | Heap | Index + TOAST | Rows |
| --- | --- | --- | --- | --- |
| `retrieval_embeddings` | 7.3 GB | 201 MB | 7.1 GB | 568,170 |
| `events` | 7.2 GB | 2.0 GB | 5.2 GB | 2,099,476 |
| `timeline_turns` | 5.9 GB | 23 MB | 5.9 GB | 21,008 |
| `event_blocks` | 3.2 GB | 609 MB | 2.6 GB | 1,184,852 |
| `code_symbols` | 3.1 GB | 2.0 GB | 1.1 GB | 3,983,411 |
| `timeline_operations` | 2.6 GB | 547 MB | 2.0 GB | 436,771 |
| `code_references` | 960 MB | 478 MB | 482 MB | 3,088,036 |
| `retrieval_embedding_sources` | 472 MB | 243 MB | 229 MB | 716,963 |
| `timeline_file_touches` | 194 MB | 85 MB | 109 MB | 359,431 |
| `repo_runtime_state` | 188 MB | 184 MB | 4 MB | 55 |
| everything else | < 60 MB combined | | | |

Inside those tables (`pg_column_size`, compressed):

- `events.payload`: 4.2 GB (Claude 1.1 GB, Codex 3.1 GB). By session idle
  age: 1.2 GB in sessions idle over 90 days, 1.7 GB idle 30 to 90 days,
  1.3 GB active in the last 30 days. Monthly inflow since May: 0.4 to
  1.1 GB.
- `event_blocks`: `tool_result.text` 883 MB, `tool_output` 286 MB,
  `tool_use.tool_input` 242 MB; natural-language `text` blocks **68 MB**.
  The two tool-result search indexes are another 1 GB.
- `timeline_turns`: `turn_json` 2.5 GB, `markdown` 1.0 GB, `chunks_json`
  74 MB; TOAST 5.8 GB against 3.5 GB live, so about 2 GB is bloat from the
  per-event delete-and-reinsert projection.
- `timeline_operations`: `result_content` 932 MB, `result_payload` 273 MB,
  `input` 243 MB, `subagent_json` 108 MB.
- `retrieval_embeddings`: each vector stored twice, `embedding REAL[]`
  1.7 GB and `embedding_vector vector(768)` 1.7 GB, plus a 2.1 GB HNSW
  index; TOAST 4.6 GB against 3.4 GB live, 60,002 dead tuples, last
  autovacuum 2026-08-28. `tool_call` chunks are 436,182 of 568,170.
- `repo_runtime_state`: 55 rows on 184 MB of heap, 8,930 dead tuples.

Sessions: 2,434 with timeline state, 2,129 idle more than 30 days.

## Context and reuse map

**Ingestion** (`backend/src/ingest/ingester.rs`): `insert_event` takes one
raw line, parses it into canonical blocks, and writes `events` plus
`event_blocks` in one transaction with `ON CONFLICT DO NOTHING` on
`(session_uuid, byte_offset)`; `rebuild_projections_after_insert` then
re-projects the session. Usage accumulates per inserted event with
`agent_usage_responses` and `last_usage_message_id` as replay dedup. The
control binary already links this module (`reset.rs`, `maintenance.rs` run
in control), so a restore can feed archived lines through `insert_event`
without a file or the ingester process.

**Rebuild paths that read `events.payload`** and must skip slimmed
sessions: `ingest/reset.rs` (`/api/admin/reindex` deletes every
`timeline_turns` and `event_blocks` row and rebuilds from payload),
`ingest/ingester/canonical_backfill.rs`, `ingest/projection.rs`,
`ingest/usage/rebuild.rs` and its four SQL files (`rebuild_legacy` deletes
all daily usage rows when upgrading from version 0), `ingest/usage/records.rs`,
`ingest/timeline/load.rs`, `model_switches.rs`, `api/timeline_routes.rs`
(Codex subagent filter), `pty/mod.rs` (`MAX(timestamp)` for the current
session), `api/stats.rs`. Without a guard, the admin reindex would erase the
timelines of every slimmed session. This is the central correctness
constraint of the plan.

**Retrieval** (`backend/src/retrieval/`): lexical search reads
`event_blocks.text` joined to `events` (speaker, timestamp) and
`timeline_turns` (preview). Semantic search returns `retrieval_embeddings`
rows and loads text from `event_blocks` or `timeline_operations`; the indexer
marks a source deleted only when a backfill revisits it. `file-history`
reads `timeline_file_touches` and `timeline_turns` (`end_timestamp`,
`preview`); `turn` reads `preview` and `markdown`; evidence packets read
operation names, categories, error flags, and file touches. Reset truncates
only the three embedding tables.

**Metrics** (`backend/src/metrics.rs`, `metrics/usage.sql`, `metrics/flow.rs`):
cost from `agent_model_usage_daily` joined to `claude_sessions` lineage,
`agent_session_metadata`, `pty_sessions.repo`, `repo_runtime_state`; churn
from `timeline_file_touches` and `timeline_turns` over 7 days; flow from
`plans`, `plan_phases`, `plan_events`; git activity from a JSON column on
`repo_runtime_state`.

**Control process** (`backend/src/main.rs`): owns migrations, startup
maintenance, the admin routes, `ingest_jobs`, and already spawns one
background loop (`submitted_prompts::run_reconciler`). The archive cycle is
a second loop of the same shape.

**Identity and AWS**: the backend container enrolls as
`spiffe://ahara/sulion/backend` through `truenas-roles-anywhere-bootstrap`
and gets role `ahara-machine-sulion-backend` from this repository's
`infrastructure/terraform/workload_identities.tf` (ahara-infra's
`machine-role` module, no extra policy today) under the boundary
`pb-sulion-truenas-workload`. That boundary already allows `s3:PutObject`,
`GetObject`, `DeleteObject`, `ListBucket`, and multipart actions on buckets
named `sulion-*` or `ahara-sulion-*`, and KMS only `ViaService ssm`, so
SSE-S3 needs no cross-repo change and SSE-KMS does. The backend image
(`rockylinux:10`) carries `awscli2` and the `postgresql` client package; the
server is PostgreSQL 18.4, so the client major must be checked in M0.
Precedents: `ahara-trust:hosts/trust/secret-backup.nix` (S3 upload under
machine identity, versioning as history, ADR-0003) and the `house-sensors`
raw-archive job (export a window, upload with metadata, then delete).

**Existing retention**: `submitted_prompts` pruned at 7 days. Nothing else.
No logical Postgres backup exists anywhere in `ahara-infra`.

## Table analysis

Every live table in `backend/migrations` (55; `timeline_references`,
`timeline_search_documents`, `control_tunnel` are dropped). "Slim" means
"for sessions whose S3 export is verified and whose grace period has
passed", never a global delete.

### Transcript-derived, large: slim per archived session

| Table | What it holds | Action | Why |
| --- | --- | --- | --- |
| `events` | one row per transcript line; `payload` is the line as JSONB | Delete rows that carry no kept text block. For the rest set `payload = NULL`, `search_text = ''`; keep session, offset, timestamp, kind, agent, speaker, content kind, event/parent/tool-use ids, flags. Needs `payload` nullable. | `payload` is 4.2 GB and the archive source. Kept rows are the join target lexical search needs for speaker and timestamp. |
| `event_blocks` | canonical blocks | Keep `kind = 'text'` blocks whose speaker is assistant, user, or summary (68 MB today). Delete thinking, tool_use, tool_result, unknown. | Kept blocks are what semantic and lexical search index as natural language. Tool output is redundant with the archive and the code index. |
| `timeline_turns` | one row per turn | Keep the row. Set `markdown = ''`, `turn_json = '{}'`, `chunks_json = '[]'`. | `timeline_file_touches` cascades from it; file-history, churn, evidence, and the turn list read its small columns. The three big columns are 3.5 GB. |
| `timeline_operations` | one row per tool call | Keep names, category, type, `pair_id`, flags. NULL `input`, `result_payload`, `subagent_json`; keep `result_content` only for errors and `name = 'agent'`, truncated to 4000 characters. | Facets, evidence, and error search need the small columns. Kept results match what the embedding indexer already keeps. |
| `agent_usage_responses` | replay dedup ids | Delete for slimmed sessions. | Only matters while the session receives events; rebuilt on restore. |
| `retrieval_embeddings`, `retrieval_embedding_sources` | vectors and source keys | In the same transaction delete sources and embeddings whose source row no longer exists (`operation_call`, `operation_result`, deleted event blocks). Embeddings of kept text stay valid. | `tool_call` chunks are 77% of a 7.3 GB table. The indexer would not notice until a forced backfill. |

### Transcript-derived, small: keep as the session skeleton

`claude_sessions` (add `archived_at`, `archive_key`, `archive_sha256`,
`archive_bytes`, `archive_events`, `purged_at`), `ingester_state`
(unchanged; offset equals file length so a still-present file is not
re-read), `agent_session_metadata`, `agent_session_usage`,
`timeline_session_state` (`latest_event_at` is the idle clock),
`timeline_file_touches` (359,431 rows, 194 MB; churn velocity and
file-history; a daily rollup is not worth it at this size),
`timeline_activity_signals`, `agent_model_switches`.

### Aggregates: keep forever

`agent_usage_daily`, `agent_model_usage_daily`. After `payload` is gone for
a session these cannot be rebuilt except by restoring, so the usage rebuild
must never delete rows for slimmed sessions.

### Durable application state: never purge, always in the dump

`pty_sessions`, `repos`, `meta_repos`, `meta_repo_members`, `workspaces`,
`workspace_dirty_paths`, `repo_runtime_state`, `repo_dirty_paths`, `plans`,
`plan_phases`, `plan_attachments`, `plan_events`, `plan_branch_anchors`,
`session_activity_state`, `library_entries`, `future_prompts`,
`future_prompt_session_state`, `device_pairings`, `device_tokens`,
`dev_nodes`, `dev_node_enrollment_tokens`, `control_identity`, `control_tls`,
`tool_category_rules`, `ingest_projection_versions`, `usage_backfills`.

### Operational logs: prune by age

`retrieval_embedding_backfills` (finished rows older than 30 days),
`ingest_jobs` (finished rows older than 90 days), `submitted_prompts`
(already 7 days).

### Code intelligence: out of scope

`code_roots`, `code_files`, `code_symbols`, `code_references`,
`code_imports`, `code_index_jobs` derive from source checkouts and are
rebuilt by the code-intelligence worker. Excluded from the dump.

## Design

### Archive object layout

```text
s3://ahara-sulion-archive-<account>/
  sessions/<agent>/<yyyy>/<mm>/<session_uuid>.jsonl.zst
  db/sulion-durable-<yyyy-mm-dd>.dump
  manifests/<yyyy-mm>/<run-id>.json
```

A session object is one JSON line per `events` row in `byte_offset` order,
which is the transcript's own line order, serialised from `payload`. Object
metadata: `session_uuid`, `agent`, `parent_session_uuid`, `event_count`,
uncompressed `sha256` and length, run id. Claude subagent sessions are their
own objects (they are their own sessions in the database) and are archived
together with their parent. The month in the key is the session's first
event.

JSONB serialisation loses key order, whitespace, and numeric formatting of
the original line. That is harmless: the only consumer is `insert_event`,
which parses JSON, and restore always replays from a clean session.

Bucket: created by this repository's Terraform as
`ahara-sulion-archive-<account>` so the existing boundary covers it.
Versioning on, public access blocked, TLS-only policy, lifecycle: Glacier
Instant Retrieval after 30 days (millisecond restore), noncurrent versions
expire after 90 days, current objects never expire. At today's rate the
bucket grows by well under 200 MB a month after zstd.

### Archive loop in the control process

A background task in the control binary, next to the submitted-prompts
reconciler, using the backend's existing machine identity plus one S3 policy
statement on its role. It polls `archive_requests` for on-demand work and
runs the monthly cycle when the last completed cycle is older than
`SULION_ARCHIVE_INTERVAL_DAYS` (default 30). Progress goes to `ingest_jobs`
so the Jobs panel shows it. Object storage sits behind a small trait with an
S3 implementation (`aws-sdk-s3`, reading the profile the bootstrap writes)
and a local-directory implementation for integration tests.

Why control rather than a new service: it already owns the migrations, the
admin reindex, the startup repairs, and `ingest_jobs`; the archive cycle is
Postgres-to-S3 with no file access; and the identity, image, and compose
wiring already exist. A separate `archiver` container would buy a narrower
IAM role at the cost of a fifth control-plane service, a new workload
declaration in `ahara-trust`, and a new Terraform role. Decision 4 records
that trade.

### Monthly cycle

1. **Durable dump.** `pg_dump -Fc` of the durable and skeleton tables only
   (`--exclude-table` for `events`, `event_blocks`, `timeline_turns`,
   `timeline_operations`, `timeline_file_touches`, `timeline_activity_signals`,
   `retrieval_*`, `code_*`), streamed to `db/`. Transcript content is covered
   by the per-session objects, and at 30 GB a monthly full dump would be
   mostly the derived tables this plan is deleting. The cycle stops if the
   dump upload fails.
2. **Select sessions.** Eligible when `timeline_session_state.latest_event_at`
   is older than `SULION_ARCHIVE_MIN_IDLE_DAYS` (default 30), the session is
   not any live PTY's `current_session_uuid`, `archived_at` is NULL or the
   session gained events since `archived_at` (then re-export replaces the
   object), and its subagent children are eligible too.
3. **Export and verify.** Stream the session's payloads as lines through
   zstd to S3, `HEAD` the object, compare length and sha256, write the
   archive columns on `claude_sessions`. Sessions with no events are skipped.
4. **Slim** sessions with `archived_at` older than
   `SULION_ARCHIVE_SLIM_AFTER_DAYS` (default 90), `purged_at IS NULL`, and no
   events newer than `archived_at`; one session per transaction in the order
   of the table analysis, ending with `purged_at = NOW()`.
5. **Prune** backfills and jobs by age.
6. **Manifest** upload; the job completes with counts and reclaimed bytes.

The grace between export and slim keeps full text in the database for
recent history and gives a bad month of uploads time to be noticed.

### Guards required before slimming ships

- Every rebuild in the reuse map filters on `claude_sessions.purged_at IS NULL`
  and, where it reads `payload` directly, `payload IS NOT NULL`.
  `rebuild_ingest_derivatives` changes from global deletes to per-session
  deletes over non-slimmed sessions.
- The usage `rebuild_legacy` scopes every `DELETE` and rebuild `INSERT` to
  non-slimmed sessions.
- **Late appends.** A slimmed session can still receive events if a Codex
  rollout is resumed after the grace (Claude's files are gone by then).
  `process_file` in the ingester checks `purged_at` before inserting; if
  set, it enqueues an `archive_requests` restore for the session and leaves
  the offset alone. The restore replays the archive, clears `purged_at`, and
  the next tick ingests the appended lines normally.
- The retrieval `turn` route and the timeline API return `archived: true`
  with the archive date for slimmed sessions; the timeline pane shows one
  banner with a restore hint instead of an empty turn body.

### Restore and replay

`sulion archive restore --session <uuid>` (also `--month`, `--repo`,
`--all`) reaches control over HTTP like `sulion plan`, inserts an
`archive_requests` row, and the loop:

1. Fetches the object, verifies sha256 against the archive columns.
2. In one transaction per session: snapshots usage totals; deletes the
   session's `events` (cascading blocks), `timeline_turns` (cascading
   operations, touches, signals), usage rows, retrieval sources and
   embeddings; clears `purged_at`.
3. Replays every line through `insert_event` with byte offsets computed
   from the reconstructed lines, then `rebuild_session_projection` and the
   usage path, exactly as the ingester would for a file. No transcript file
   is written.
4. Compares recomputed usage totals with the snapshot and records any
   difference as a job warning.
5. With `--purge-after` (default for `--all`), re-slims immediately: the
   object already exists and matches, so nothing is uploaded.

A whole-history re-index is `--all --purge-after`, oldest month first, with
a bound on concurrently restored sessions so the database does not grow by
the whole archive at once. `/api/admin/reindex` remains the cheap rebuild
for non-slimmed sessions.

Restoring the durable dump is an operator runbook (`pg_restore` into the
TrueNAS instance followed by `--all` session restore for whatever history is
wanted), documented in M5.

### Reclaim bloat before slimming anything

About 3 GB of the 30 GB is dead space: 2 GB of TOAST in `timeline_turns`
from the projection's rewrite-per-event, about 1.2 GB in
`retrieval_embeddings` (stale autovacuum, 60,002 dead tuples), 184 MB of
heap for 55 rows in `repo_runtime_state`. `VACUUM (FULL)` on those three
tables recovers it with no row change; it takes an exclusive lock, so it
runs in a control-plane maintenance window with the ingester and retrieval
indexer quiet. Per-table `autovacuum_vacuum_scale_factor` settings go in
the M3 migration so the bloat does not return.

### Consequences to accept

- Timeline detail and full-text search for slimmed sessions cover only
  assistant, user, and summary text, error results, and subagent finals
  until restored.
- The backend's role gains S3 put, get, and list on one bucket. The
  boundary already permitted it; the role policy makes it explicit.
- Restoring a session recomputes its usage rows; if parsing or dedup logic
  changed since the original ingest, the recomputed cost can differ, which
  the job reports.

## Decisions

Settled by this analysis:

- Archive source is `events.payload`; unit is the agent session; files on
  the node are out of scope.
- Slimming is per session, gated on a verified S3 object, and keeps every
  table's skeleton plus natural-language text; daily usage tables are never
  rebuilt for slimmed sessions.
- Restore replays through `insert_event` in the control process; no
  transcript file is written and the ingester is unchanged apart from the
  late-append guard.
- Dump covers durable and skeleton tables only.

`[DECISION]` user-owned, blocking M1:

1. **Object encryption.** SSE-S3 works inside today's boundary; SSE-KMS
   with a project key needs a `kms ViaService s3` statement added to
   `pb-sulion-truenas-workload` in `ahara-infra`. Recommendation: SSE-S3.
2. **Windows.** Recommendation: export after 30 idle days, slim 90 days
   after export, cycle every 30 days. All env-tunable.
3. **Authorize the M0 `VACUUM (FULL)` maintenance** in a window you choose.
   Recommendation: yes; about 3 GB back with no data change.
4. **Where the loop runs.** Inside the control process on the backend's
   identity (recommended), or as a separate `archiver` control-plane service
   with its own `spiffe://ahara/sulion/archiver` identity.

`[DECISION]` deferred, non-blocking:

5. **Drop `retrieval_embeddings.embedding REAL[]`** now that pgvector is
   present (the `embedding_vector` column exists). Saves 1.7 GB plus TOAST;
   needs the non-pgvector fallback in the retrieval service retired or made
   conditional. Proposed for M3; owner: user.
6. **Dump retention.** Default: keep every monthly durable dump; they are
   small.
7. **Codex rollouts on the node.** Out of scope now; once restore works,
   deleting rollouts whose session is archived is a ten-line addition to
   the ingester host if disk ever matters. Owner: user; trigger: node disk
   pressure.

Provisional assumptions, checked in M0:

- The backend image's `postgresql` client is major 18 or can be replaced by
  the pgdg `postgresql18` package; a 16-series `pg_dump` refuses an 18
  server.
- `sulion_app_app` can dump the tables it owns (it runs the migrations).
- `aws-sdk-s3` honours the `credential_process` profile the bootstrap writes
  (the same mechanism the CLI uses today).

## Milestones

### M0 — Settle decisions, maintenance, tool checks

Scope: decisions 1 to 4; authorized `VACUUM (FULL)` on `timeline_turns`,
`retrieval_embeddings`, `repo_runtime_state`; `pg_dump --version` in the
backend image against 18.4 and the image change if needed; trial
`pg_dump -Fc` of the durable table set to size it.
Acceptance: decisions recorded here; `pg_database_size` before and after
maintenance recorded; dump size and client version recorded.
Evidence: `psql` through `with-cred --`; the trial dump byte count.

### M1 — Bucket and role  [depends on M0 decisions 1 and 4]

Scope: Terraform in this repository for the bucket (versioning, public
block, TLS-only, SSE, lifecycle) and an S3 policy on the chosen role;
control gets `SULION_ARCHIVE_BUCKET` in compose; a startup probe that logs
whether the bucket is reachable.
Acceptance: control logs a successful `HEAD` on the bucket after deploy.
Evidence: `terraform plan` reviewed before apply; control logs.

### M2 — Session export and guards  [depends on M1]

Scope: migration for the `claude_sessions` archive columns and
`archive_requests`; object-store trait with S3 and directory
implementations; the archive loop with monthly scheduling, eligibility,
streaming export, verify, manifest, `ingest_jobs` progress, dry run;
`sulion archive run|status|list` CLI; `purged_at` and `payload IS NOT NULL`
guards in every rebuild path even though nothing is slimmed yet.
Acceptance: a dry run lists the eligible set; a real run exports every idle
session, and `HEAD` counts match `claude_sessions.archived_at` counts.
Evidence: integration tests with the directory store covering eligibility
(live PTY exclusion, parent and child coupling, re-export after new events),
verify-before-mark, sha mismatch refusal; registered in
`scripts/run-backend-integration-tests.sh`.

### M3 — Durable dump and per-session slimming  [depends on M2]

Scope: dump step gating the cycle; migration dropping `NOT NULL` on
`events.payload` and setting autovacuum factors; slimming transaction;
retrieval source cleanup; age pruning; late-append guard in the ingester;
`archived` flag on the timeline API and retrieval `turn`; timeline banner;
if decision 5 is yes, drop the `REAL[]` column.
Acceptance: after slimming a session, cost report, churn, file-history,
lexical and semantic search over natural-language text, evidence packets,
and the turn list return the same rows; turn detail shows the banner;
`/api/admin/reindex` and the usage rebuild leave the session untouched; an
append to a slimmed Codex session triggers a restore instead of a broken
projection.
Evidence: integration tests asserting each consumer before and after
slimming; a startup-repair test with a bumped projection version over a mix
of slimmed and live sessions; an ingester test for the late-append guard.
Expected reclaim at today's sizes for sessions idle over 90 days: 1.2 GB of
payload, most of their blocks and operations, their turn renderings, and
their tool-call embeddings; the bulk of the 30 GB follows as the 30-to-90
day cohort ages.

### M4 — Restore and replay  [depends on M3]

Scope: `sulion archive restore` with session, month, repo, and all scopes;
fetch and verify; per-session reset transaction; replay through
`insert_event`; projection and usage rebuild; usage comparison;
`--purge-after`; concurrency bound for `--all`.
Acceptance: restoring a slimmed session reproduces its timeline,
operations, usage, and embeddings; `--all --purge-after` over a small
archive ends with the same database state it started from.
Evidence: integration test performing export → slim → restore → compare
with the directory store; a control-plane run over one real month.

### M5 — Documentation and runbooks  [depends on M4]

Scope: `docs/deploy.md` (bucket, loop, restore runbook, `pg_restore`
procedure), `docs/architecture.md` (invariants: slim only after a verified
archive; rebuilds skip slimmed sessions), `docs/ingestion.md` (archive
columns, replay, late-append guard), an ADR for tiered retention,
`CHANGELOG.md`.
Acceptance: a reader can run a full re-index from S3 and a durable restore
from the docs alone.

## Sulion mapping

Root plan `c30ad1cf-1554-4ab8-8c87-11fe57575754` (all phases pending).
The revision 1 plan `c4bbe19d-0971-4133-933a-805959c9cd0a` is closed as
canceled, superseded by this one.

| Milestone | Phase ID |
| --- | --- |
| M0 — Settle decisions, maintenance, tool checks | `7409682d-3dab-4da9-83c7-0493989c2c0e` |
| M1 — Bucket and role | `d99ba632-e245-463f-9c4f-56de37815c4f` |
| M2 — Session export and guards | `551a9287-ee88-4a51-850f-ec7dcec8a8e5` |
| M3 — Durable dump and per-session slimming | `177afdc2-3882-4504-a38b-5e913c435b82` |
| M4 — Restore and replay | `445c7322-2d81-4df1-9a4b-37256a97c0f9` |
| M5 — Documentation and runbooks | `1d37f8cb-ac3c-4cac-a1a1-bf2a80076d64` |

## Current state

Completed: exploration, database and disk measurement (2026-09-19), this
plan (revision 2, database-focused). Remaining: M0 through M5, all pending.
Blockers: decisions 1 to 4. Next action: user answers them; then M0
maintenance in a window they choose.
