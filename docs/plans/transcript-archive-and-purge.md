# Database archive, backup, and monthly purge

Plan only. No code, schema, infrastructure, or database change has been made.
Status: proposal; design settled, awaiting authorization to execute M0.

Documentation review, 2026-09-21: M0–M5 remain pending. The measured state below
is the September 19 baseline, not a current database inventory. Since then,
`94056c8` dropped the unused `timeline_turns.turn_json` column and `c3fe6a5`
reduced markdown to prompt, assistant text, and tool headers. Both retain
`events.payload`, the archive source. Re-measure digest size, table bloat, and
post-purge estimates in M0; those changes do not implement archival or authorize
purging, S3 publication, or maintenance.

Revision 3 (2026-09-19). Revision 2 moved scope from JSONL files to the
database. Revision 3 redefines what survives a purge from the user's side:
what someone can still do with history older than the purge, and nothing
kept that does not serve one of those uses.

## Outcome and scope

The September 19 sample measured a 30 GB database and roughly 1 GB of new
transcript payload a month, with large derived copies. This proposal adds
a control-plane archive cycle that:

1. **Exports each idle session to S3** as one JSON-lines object
   reconstructed from `events.payload`, under the backend's machine identity
   from the trust appliance. The database is already the only complete copy
   of Claude history (see "Why the database, not the files").
2. **Dumps the durable tables** (plans, sessions, settings, rollups) to S3
   on the same cycle.
3. **Purges archived sessions down to a turn digest**: one row per turn
   holding the prompt, the rendered markdown, timestamps, tokens, and the
   files it touched, plus one embedding set per turn. Everything else for
   that session goes. Cost and file churn are **rolled up** to daily,
   repo-level tables before the per-session rows go.
4. **Restores any session from S3 on request**, replaying its lines through
   the normal ingest path, and re-purges it afterwards. A whole-history
   re-index is the same operation batched by month.

Non-goals: JSONL files on the node, Codex's own state, the
code-intelligence tables, public edge work.

## What you can do with archived history

This is the contract the table plan below serves. "Archived" means the
session's export is verified in S3 and its grace period has passed.

| Use | Live history | Archived history |
| --- | --- | --- |
| `sulion-retrieve search` | hits on assistant/user text, tool calls, tool errors, with per-turn evidence | hits on turns: prompt and rendered markdown, lexical and semantic; evidence is the turn's file list |
| `sulion-retrieve turn` | compact turn digest | the same compact turn digest |
| `sulion-retrieve file-history <path>` | every turn that touched the path, with preview and time | same, from the turn digest's file list |
| Metrics: cost by day, repo, agent, model | from per-session daily rows | from the daily rollup, same numbers |
| Metrics: file churn velocity | from per-turn touches | from the daily file-activity rollup; hotspots keep the 7-day live window |
| Metrics: plan flow, git activity | unchanged | unchanged |
| Timeline pane | turn list plus expandable tool operations | turn list plus markdown detail with an "archived" banner; no tool expansion |
| `--resume` in the agent CLI | if the harness still has the file | no |
| Anything else (tool output, thinking, operation-level search) | yes | restore the session first |

The digest is what `sulion-retrieve turn` returns: the prompt, every assistant
text block, and one header per tool call, rendered by
`ingest/timeline/render.rs`. It excludes tool inputs, reconstructed diffs, and
result bodies. The September 19 measurement of 1.0 GB predates this reduction
and must not be used as its current size.

## Why the database, not the files

Measured 2026-09-19:

- Claude Code deletes its own transcripts after 30 days
  (`~/.claude/.last-cleanup`, no `cleanupPeriodDays` override). 745 Claude
  sessions exist only in the database today.
- Every rebuild path, including `/api/admin/reindex`, the
  projection-version startup repairs, and the retrieval reset, reads
  `events.payload`. None reads JSONL, and no code path deletes `events`,
  `claude_sessions`, or `ingester_state` rows. Vacuumed sessions survive
  every reindex today.
- The database is 30 GB; all JSONL on disk is 5.6 GB, 5.2 GB of it Codex.

## Measured state — 2026-09-19, before projection reductions

Disk on the node: `~/.claude/projects` 384 MB (233 files),
`~/.codex/sessions` 5.2 GB (1465 rollouts since April). `ingester_state`
against disk: 1689 present and fully ingested, 0 partial, 223 missing (all
Claude). Another 552 Claude sessions have events but no state row and no
file. Database-only total: 745 Claude sessions, 333,312 events, about 820 MB
compressed payload.

Database, PostgreSQL 18.4, 30 GB:

| Table | Total | Rows | Notes |
| --- | --- | --- | --- |
| `retrieval_embeddings` | 7.3 GB | 568,170 | vector stored twice (`REAL[]` 1.7 GB, pgvector 1.7 GB), HNSW 2.1 GB, ~1.2 GB TOAST bloat, 60k dead tuples; 77% are `tool_call` chunks |
| `events` | 7.2 GB | 2,099,476 | `payload` 4.2 GB compressed: 1.2 GB in sessions idle > 90 d, 1.7 GB idle 30–90 d, 1.3 GB active |
| `timeline_turns` | 5.9 GB | 21,008 | `turn_json` 2.5 GB, `markdown` 1.0 GB, `chunks_json` 74 MB; ~2 GB TOAST bloat |
| `event_blocks` | 3.2 GB | 1,184,852 | tool result/output/input 1.4 GB; natural-language text 68 MB; tool-result indexes 1 GB |
| `code_symbols` + `code_references` | 4.1 GB | | code index, out of scope |
| `timeline_operations` | 2.6 GB | 436,771 | `result_content` 932 MB, `result_payload` 273 MB, `input` 243 MB |
| `retrieval_embedding_sources` | 472 MB | 716,963 | |
| `timeline_file_touches` | 194 MB | 359,431 | |
| `repo_runtime_state` | 188 MB | 55 | bloat |
| everything else | < 60 MB | | |

Sessions: 2,434 with timeline state, 2,129 idle more than 30 days. Monthly
payload inflow since May: 0.4 to 1.1 GB.

## Context and reuse map

**Ingestion** (`backend/src/ingest/ingester.rs`): `insert_event` takes one
raw line and writes `events` plus `event_blocks` with `ON CONFLICT DO
NOTHING`; `rebuild_projections_after_insert` re-projects the session. Usage
accumulates per inserted event. The control binary links this module, so a
restore can replay archived lines through `insert_event` without a file.

**Rebuild paths that read `events.payload`** and must skip archived
sessions: `ingest/reset.rs` (`/api/admin/reindex` deletes every
`timeline_turns` and `event_blocks` row and rebuilds from payload),
`ingest/ingester/canonical_backfill.rs`, `ingest/projection.rs`,
`ingest/usage/rebuild.rs` and its SQL (`rebuild_legacy` deletes all daily
usage rows on upgrade from version 0), `ingest/usage/records.rs`,
`ingest/timeline/load.rs`, `model_switches.rs`, `api/timeline_routes.rs`,
`pty/mod.rs`, `api/stats.rs`. Without a guard, the admin reindex would erase
the digest of every archived session.

**Retrieval** (`backend/src/retrieval/`): lexical search today reads
`event_blocks.text`; semantic search returns `retrieval_embeddings` and
loads text from blocks or operations; `turn` reads `timeline_turns.markdown`;
`file-history` reads `timeline_file_touches` joined to turns; evidence
reads operations and touches. The source enum already includes
`turn_digest`, unused today. Reset truncates only the embedding tables.

**Metrics** (`backend/src/metrics.rs`, `metrics/usage.sql`): cost joins
`agent_model_usage_daily` to `claude_sessions` lineage, `agent_session_metadata`,
`pty_sessions.repo`, and `repo_runtime_state` to attribute a repo; churn
hotspots read touches over 7 days; flow reads plan tables.

**Timeline UI** renders turn detail from `chunks_json` and operation rows
(`frontend/src/components/timeline/TurnDetail.tsx`); markdown is not used
by the UI today.

**Control process** already runs one background loop
(`submitted_prompts::run_reconciler`) and owns migrations, startup repair,
admin routes, and `ingest_jobs`.

**Identity and AWS**: the backend enrolls as `spiffe://ahara/sulion/backend`
with role `ahara-machine-sulion-backend` (this repository's
`infrastructure/terraform/workload_identities.tf`) under the boundary
`pb-sulion-truenas-workload`, which already allows S3 put/get/delete/list on
buckets named `ahara-sulion-*` and KMS only via SSM. The backend image
(`rockylinux:10`) has `awscli2` and the stock `postgresql` client; the
server is 18.4. No logical Postgres backup exists in `ahara-infra`.

## Table plan

Per archived session unless marked otherwise.

### Rolled up, then purged

| Table | Rollup | Then |
| --- | --- | --- |
| `agent_model_usage_daily`, `agent_usage_daily`, `agent_session_usage`, `agent_usage_responses` | New `usage_daily_rollup(day, repo, agent, model, standard_input, cache_read, cache_write, cache_write_1h, output)`, written at archive time with the same repo attribution `metrics/usage.sql` performs today (PTY repo, lineage, project-hash fallback). | Delete the session's rows. The metrics query becomes the union of the rollup and live per-session rows. |
| `timeline_file_touches` | New `file_activity_daily(repo, path, day, write_turns, read_turns, sessions)`; and each archived turn's touched paths fold into `timeline_turns.files_json` (`[{path, write}]`, GIN-indexed) so `file-history` can still list archived turns for a path. | Delete the session's rows. Churn velocity reads the rollup; the 7-day hotspot query is unaffected because it only looks at live history. |

### Reduced to the turn digest

| Table | Keep | Drop |
| --- | --- | --- |
| `timeline_turns` | `preview`, `user_prompt_text`, `markdown`, timestamps, `duration_ms`, counts, `has_errors`, tokens, sidechain flag, new `files_json` | `chunks_json`; `turn_json` was already dropped in `94056c8` |
| `retrieval_embeddings`, `retrieval_embedding_sources` | new `turn_digest` sources, one per archived turn, embedding the markdown in chunks under the existing chunking rules | every block-level and operation-level source and embedding for the session |

### Purged entirely

`events` (the archive source; gone once the object is verified),
`event_blocks`, `timeline_operations`, `timeline_activity_signals`,
`agent_model_switches`, `ingester_state` (a file that reappears triggers
the late-append guard through `purged_at`, not through state).

### Kept as the session skeleton (tiny)

`claude_sessions` with new `archived_at`, `archive_key`, `archive_sha256`,
`archive_bytes`, `archive_events`, `purged_at`; `agent_session_metadata`
(model and cwd give search its repo scope); `timeline_session_state`
(`latest_event_at` is the idle clock).

### Never purged, always in the dump

`plans`, `plan_phases`, `plan_attachments`, `plan_events`,
`plan_branch_anchors`, `pty_sessions`, `repos`, `meta_repos`,
`meta_repo_members`, `workspaces`, `workspace_dirty_paths`,
`repo_runtime_state`, `repo_dirty_paths`, `session_activity_state`,
`library_entries`, `future_prompts`, `future_prompt_session_state`,
`device_pairings`, `device_tokens`, `dev_nodes`, `dev_node_enrollment_tokens`,
`control_identity`, `control_tls`, `tool_category_rules`,
`ingest_projection_versions`, `usage_backfills`, and the two new rollups.

### Pruned by age

`retrieval_embedding_backfills` (finished, 30 days), `ingest_jobs`
(finished, 90 days), `submitted_prompts` (already 7 days).

### Out of scope

`code_*` (4 GB): rebuilt from source by the code-intelligence worker;
excluded from the dump.

### What the database looks like after a full purge of everything older than the grace

The original estimate used digest markdown about 1 GB, turn-digest embeddings
well under 1 GB, the two rollups and skeletons a few tens of MB, plus the live window (sessions
younger than idle + grace, about 3 GB of payload and projections at
September 19 rates), giving roughly 5 to 6 GB of transcript history instead
of 26 GB. Recompute this after the projection reductions. Digests, embeddings,
rollups, and skeletons still grow with retained history; purging bounds the
detailed live window, not the entire database.

## Design

### Archive object layout

```text
s3://ahara-sulion-archive-<account>/
  sessions/<agent>/<yyyy>/<mm>/<session_uuid>.jsonl.zst
  db/sulion-durable-<yyyy-mm-dd>.dump
  manifests/<yyyy-mm>/<run-id>.json
```

One JSON line per `events` row in `byte_offset` order, serialised from
`payload`. Metadata: session, agent, parent session, event count, sha256
and length of the uncompressed bytes, run id. Subagent sessions are their
own objects and move with their parent. JSONB re-serialisation changes key
order and number formatting, which `insert_event` does not care about.

Bucket `ahara-sulion-archive-<account>` from this repository's Terraform:
versioning, public access blocked, TLS-only, SSE, Glacier Instant Retrieval
after 30 days, noncurrent versions expire after 90 days, current objects
never. Well under 200 MB a month after zstd.

### Archive loop in the control process

A background task next to the submitted-prompts reconciler, using the
backend's identity plus one S3 policy statement. Polls `archive_requests`
for on-demand work; runs the cycle when the last completed one is older than
`SULION_ARCHIVE_INTERVAL_DAYS` (30). Progress in `ingest_jobs`. Object
storage behind a trait with S3 (`aws-sdk-s3`) and local-directory
implementations; tests use the directory.

### Monthly cycle

1. **Durable dump.** `pg_dump -Fc` of the never-purged tables, the
   skeletons, and the rollups (excluding `events`, `event_blocks`,
   `timeline_*`, `retrieval_*`, `code_*`). Stop if the upload fails.
2. **Select.** `timeline_session_state.latest_event_at` older than
   `SULION_ARCHIVE_MIN_IDLE_DAYS` (30); not a live PTY's current session;
   not yet archived, or archived but with events newer than `archived_at`;
   subagent children eligible with their parent.
3. **Export and verify.** Stream lines through zstd, `HEAD`, compare
   length and sha256, write the archive columns.
4. **Purge** sessions with `archived_at` older than
   `SULION_ARCHIVE_PURGE_AFTER_DAYS` (90) and no newer events, one session
   per transaction: write the usage rollup rows; write the file-activity
   rollup rows and `files_json`; enqueue `turn_digest` embedding sources;
   delete the block and operation sources and embeddings; delete the
   per-session usage rows, touches, signals, operations, blocks, events,
   switches, ingester state; null `chunks_json`; set
   `purged_at`. The retrieval indexer embeds the digests on its next drain.
5. **Prune** by age; **manifest**; job complete with reclaimed bytes.

### Guards before purging ships

- Every rebuild in the reuse map filters `claude_sessions.purged_at IS NULL`
  and `payload IS NOT NULL`; `rebuild_ingest_derivatives` moves from global
  deletes to per-session deletes over live sessions; the usage rebuild never
  touches rollup rows.
- **Late appends.** A purged Codex session can still be resumed and grow.
  `process_file` checks `purged_at` before inserting; if set, it enqueues a
  restore and leaves the offset alone. After restore the next tick ingests
  the new lines normally.
- Search for archived sessions: lexical over `timeline_turns.markdown` and
  `user_prompt_text` (trgm or FTS index on markdown, sized in M0), semantic
  over `turn_digest` embeddings; results carry `archived: true` and the
  file list as evidence. `turn` is unchanged. `file-history` unions touches
  and `files_json`.
- Timeline API returns `archived: true`; the pane renders the turn list as
  today and turn detail from markdown with a banner and a restore hint.

### Restore and replay

`sulion archive restore --session <uuid>` (also `--month`, `--repo`,
`--all`) inserts an `archive_requests` row; the loop fetches and verifies
the object, deletes the session's digest embeddings and rollup
contributions (rollups carry `session_uuid` in a side table so a restore
can subtract exactly what it added), clears `purged_at`, replays every line
through `insert_event` with offsets computed from the lines, rebuilds the
projection and usage, compares recomputed usage with the rollup it
subtracted, and with `--purge-after` (default for `--all`) purges again
without re-uploading. `--all --purge-after` walks months oldest first with
a concurrency bound.

### Bloat maintenance first

The September 19 sample estimated about 3 GB of dead space (`timeline_turns`
~2 GB TOAST, `retrieval_embeddings` ~1.2 GB, `repo_runtime_state` 184 MB heap for 55
rows). Re-measure before requesting the maintenance window; the later column
drop and re-render change this baseline. Any `VACUUM (FULL)` remains subject
to decision 3 below. Per-table autovacuum factors are proposed for M3.

### Consequences to accept

- Archived turns have no tool output, thinking, or per-operation detail
  until restored. Search over old history is turn-grained.
- Old cost and churn numbers are exactly the rollups; per-session drill-down
  for archived sessions needs a restore.
- Restoring recomputes usage; if parsing changed since the original ingest
  the numbers can differ, which the job reports.
- The backend's role gains S3 put, get, list on one bucket.

## Decisions

Settled:

- Archive source is `events.payload`; unit is the agent session; node files
  are out of scope.
- Archived history is the turn digest plus two daily rollups; everything
  else per session is purged after a verified export.
- Restore replays through `insert_event` in the control process.
- The dump covers durable tables, skeletons, and rollups only.

Settled in revision 3 (previously listed as user decisions; none needed
user input):

- **Object encryption: SSE-S3.** Transcripts are code and prompts, the
  bucket is private, versioned, and TLS-only, and SSE-S3 needs no change
  outside this repository. Recorded in the M5 ADR; SSE-KMS can be adopted
  later by adding a `kms ViaService s3` statement to the boundary.
- **Windows: export after 30 idle days, purge 90 days after export, cycle
  every 30 days.** Env-tunable defaults; changing them is a config edit.
- **The loop runs inside the control process** on the backend's identity.
  A separate service buys a narrower IAM role at the cost of a fifth
  control-plane container, a new workload declaration in `ahara-trust`,
  and a new Terraform role.
- **Bloat maintenance is part of M0.** `VACUUM (FULL)` on
  `timeline_turns`, `retrieval_embeddings`, and `repo_runtime_state` takes
  exclusive locks on the production database, so it is scheduled with the
  user when M0 executes; that is an authorization to run, not a design
  choice.

Deferred, with trigger:

- **Drop `retrieval_embeddings.embedding REAL[]`** (1.7 GB plus TOAST) now
  that pgvector is present; needs the non-pgvector fallback retired.
  Proposed for M3.
- **Codex rollouts on the node**: once restore works, deleting rollouts for
  archived sessions is a small ingester-host addition. Trigger: disk
  pressure on the node.

What remains the user's: authorizing execution of each milestone, and
choosing the maintenance window for the M0 vacuum.

Assumptions checked in M0: the image's `pg_dump` is major 18 or is replaced
by pgdg `postgresql18`; `sulion_app_app` can dump what it owns; `aws-sdk-s3`
honours the bootstrap's `credential_process` profile; a trgm or FTS index on
1 GB of markdown is acceptable (size it on a copy).

## Milestones

### M0 — Re-measure, maintenance, tool checks

Re-measure after the projection reductions of 2026-09-21 (`turn_json`
dropped, markdown reduced to prompt, assistant text, and tool headers):
digest size, table bloat, post-purge estimate. `VACUUM (FULL)` in a window
the user picks; `pg_dump` version and image change if needed; trial durable
dump size; size the markdown search index.
Acceptance: all recorded here with before/after `pg_database_size`.

#### M0 execution steps (2026-09-22)

1. Re-measure after the projection reductions.
   - State / evidence: **blocked**. The broker's database grant for this
     PTY expired between sessions (`with-cred` now injects no
     `SULION_DB_PASSWORD`; psql reports `fe_sendauth: no password
     supplied`). Not retried beyond one attempt per the credential rule.
     Re-run once the grant is renewed; the September 19 numbers stand as the
     baseline until then.
2. `VACUUM (FULL)` on the three bloated tables.
   - State: **pending a maintenance window** named by the user; not run.
3. `pg_dump` client for an 18.4 server.
   - Evidence: the backend image (`rockylinux:10`) installs the appstream
     `postgresql` package, which is 16.14 and cannot dump an 18 server. The
     pgdg repository for EL-10 offers `postgresql18` (client 18.6),
     verified in a throwaway container. The dnf metadata signature needs the
     pgdg key imported; the Dockerfile change in M3 imports it.
   - Outcome: M3 adds the pgdg client to the image and sets
     `SULION_PG_DUMP=/usr/pgsql-18/bin/pg_dump`.
4. Trial durable dump size.
   - State: blocked on the same credential as step 1.
5. S3 client choice.
   - Evidence: the image already carries `awscli2` and the bootstrap sets
     `AWS_PROFILE`; the trust appliance's backup uses the same CLI. The
     `aws-sdk-s3` crate is not in the local registry and would add a large
     dependency set for three calls.
   - Outcome: the object store shells out to `aws s3 cp` / `aws s3api
     head-object` behind the store trait; tests use the directory store.
     Records a change from the revision 3 text, which named the SDK.

M0 acceptance is therefore partial: tool checks done, measurements and
maintenance deferred to the user's credential renewal and window. Work
continues on M1–M5, none of which depends on those numbers.

### M1 — Bucket and role  [depends on M0]

Terraform bucket and S3 policy on the chosen role; `SULION_ARCHIVE_BUCKET`
in compose; control startup probe. Acceptance: control logs a successful
`HEAD` after deploy.

### M2 — Export, rollups, guards  [depends on M1]

Migrations: archive columns, `archive_requests`, `usage_daily_rollup`,
`file_activity_daily`, `timeline_turns.files_json`. Archive loop with
export, verify, manifest, scheduling, dry run, `ingest_jobs`. Metrics read
rollup ∪ live. `purged_at`/`payload IS NOT NULL` guards everywhere.
`sulion archive run|status|list`. Acceptance: every idle session exported,
`HEAD` counts match; metrics unchanged with empty rollups. Evidence:
integration tests with the directory store (eligibility, live-PTY and
parent/child rules, verify-before-mark, sha mismatch), registered in the
harness script.

### M3 — Purge to the digest  [depends on M2]

Durable dump gate; migration for nullable `payload` and autovacuum
factors; purge transaction; `turn_digest` sources and embedding; markdown
search index; retrieval search/evidence/file-history over archived
sessions; late-append guard; `archived` flag; timeline markdown detail with
banner; `REAL[]` drop (done 2026-09-23, migrations 0090/0091). Acceptance: for a purged session, `search`
finds it and `turn` reads it, `file-history` lists its turns, cost and churn
totals are identical before and after, the timeline shows its turns,
`/api/admin/reindex` and the usage rebuild leave it alone, and an append to
a purged Codex session triggers a restore. Evidence: integration tests per
consumer before/after; startup-repair test over mixed sessions; ingester
guard test; UI test for the archived detail view.

### M4 — Restore and replay  [depends on M3]

Restore by session/month/repo/all; rollup subtraction; replay through
`insert_event`; usage comparison; `--purge-after`; concurrency bound.
Acceptance: restore reproduces timeline, operations, usage, embeddings;
`--all --purge-after` over a small archive ends where it started.

### M5 — Documentation and runbooks  [depends on M4]

`docs/deploy.md`, `docs/architecture.md` invariants (purge only after a
verified export; rebuilds skip archived sessions), `docs/ingestion.md`,
`docs/retrieval.md` (archived tier), an ADR for tiered retention,
`CHANGELOG.md`, `pg_restore` and full re-index runbooks.

## Sulion mapping

Root plan `c30ad1cf-1554-4ab8-8c87-11fe57575754`, all phases pending.
Phase IDs: M0 `7409682d-3dab-4da9-83c7-0493989c2c0e`, M1
`d99ba632-e245-463f-9c4f-56de37815c4f`, M2 `551a9287-ee88-4a51-850f-ec7dcec8a8e5`,
M3 `177afdc2-3882-4504-a38b-5e913c435b82`, M4 `445c7322-2d81-4df1-9a4b-37256a97c0f9`,
M5 `1d37f8cb-ac3c-4cac-a1a1-bf2a80076d64`. (M2/M3 titles in Sulion predate
revision 3; scope is as written here.) Revision 1 plan
`c4bbe19d-0971-4133-933a-805959c9cd0a` is canceled.

## Implementation record (2026-09-22)

M1–M5 were implemented in one working tree under the user's "implement all
phases" authorization. Nothing is pushed, applied, or deployed.

- **M1 — bucket and role.** `infrastructure/terraform/archive.tf` (bucket,
  versioning, SSE-S3, public block, TLS-only policy, lifecycle, SSM
  parameter, backend policy document); `workload_identities.tf` passes the
  policy to the backend's machine role only; `secret-paths.yml` resolves
  `SULION_ARCHIVE_BUCKET`; `compose.yaml` carries the archive env and
  `SULION_PG_DUMP`. Evidence: `terraform fmt -check`, `terraform init
  -backend=false`, `terraform validate` pass; all three compose selections
  render. The startup probe became `sulion archive status` plus the loop's
  own start log rather than a separate check.
- **M2 — export, rollups, guards.** Migration `0088_transcript_archive.sql`
  (archive columns, `archive_requests`, `archive_state`, the two rollups and
  their contribution tables, `timeline_turns.files_json`, nullable
  `events.payload`, `turn_digest` family, autovacuum factors) and
  `0089_timeline_turns_markdown_fts_idx.sql`. `backend/src/archive/`
  (`store`, `export`, `dump`, `purge`, `restore`, `requests`, loop).
  Guards: `projection/write.rs` skips purged sessions in all three rebuild
  entry points; `reset.rs` scopes its deletes to live sessions;
  `metrics/usage.sql` unions the rollup. CLI `sulion archive` over the
  correlate socket (`ControlRequest::Archive*`), REST `/api/admin/archive*`.
  Object store is the `aws` CLI or a directory (see M0 step 5).
- **M3 — dump and purge.** In the same module: `pg_dump` gate
  (`dump_enabled`), per-session purge transaction, digest embedding
  sources, late-append guard in `ingester.rs`, `archived_at` on the
  timeline summary and detail responses, retrieval `archived` flags,
  archived-tier lexical (`lexical_digest_search`) and semantic
  (`turn_digest` in the include set and text `CASE`), evidence from
  `files_json`, file-history and repo file-trace unions, the timeline
  banner and markdown rendering in `TurnDetail.tsx`. Dockerfile installs
  the pgdg 18 client. The `REAL[]` column drop followed on 2026-09-23 (see
  the follow-up record below).
- **M4 — restore.** `restore.rs` and `ingest::replay_session_lines`;
  scopes session, month, repo, all; `--purge-after`; sequential, so at most
  one restored session is live at a time.
- **M5 — docs.** `docs/ingestion.md` retention boundary, `docs/retrieval.md`
  archived sessions, `docs/architecture.md` invariant 8 and the CLI list,
  `docs/deploy.md` transcript archive and runbooks,
  `docs/adrs/0003-tiered-transcript-retention.md`, `CHANGELOG.md`.

Evidence: `backend/tests/archive_integration.rs` (registered in the
harness) covers export verification and eligibility, dry run, purge with
every consumer checked before and after (cost totals and repo attribution,
file-history and repo file-trace, digest markdown and file list, embedding
sources, admin reindex leaving the digest alone, timeline `archived_at`,
the late-append guard queuing exactly one restore), restore replaying to
identical offsets, operations, touches, and cost with the rollup subtracted,
`--all --purge-after`, the guard releasing an appended line after restore,
and the durable dump when a matching `pg_dump` is on `PATH`. All four pass
against a throwaway Postgres 16. `cargo check --all-targets` is clean;
`TurnDetail.test.tsx` gains two cases (15 pass); `tsc` and eslint pass on
the changed files. `make test-rust-integration` ran all twelve targets: every
target passed except three `retrieval_integration` cases that asserted the
number of backfill families as 3; the turn-digest family makes it 4, the
assertions were updated, and the target then passed in full. `cargo test
--lib` passes (298).

Follow-up on 2026-09-23 (user: first run must delete nothing; vacuum any
time; decide `REAL[]`):

- **Purge gate.** `archive_state.purge_enabled` starts false. A cycle
  exports and dumps but selects no purge candidates until
  `sulion archive purge-gate on` (also `POST /api/admin/archive/purge-gate`).
  `restore --all` / `--purge-after` restore without re-purging while it is
  closed and say so. `sulion archive verify [--deep]` checks every archived
  object against its row; deep re-downloads and re-hashes. Covered by a new
  integration test (tampered and missing objects, gate closed and open).
- **Re-measure (2026-09-23, before the vacuum below).** Database 25 GB:
  `events` 7.6 GB, `retrieval_embeddings` 7.3 GB (4.6 GB TOAST, 60k dead
  tuples, autovacuum still 2026-08-28), `event_blocks` 3.3 GB,
  `code_symbols` 3.1 GB, `timeline_operations` 2.2 GB, `timeline_turns`
  217 MB with 59 MB of markdown (the digest corpus is now that small).
  2,285 of 2,455 sessions idle over 30 days. pgvector present.
- **`VACUUM (FULL)`** run on `repo_runtime_state`, `workspaces`,
  `timeline_turns`, `retrieval_embeddings`; result in "Current state".
- **`REAL[]` column: dropped** (reversing the same-day decision above,
  which had kept it for the test harness's sake). Migration
  `0090_retrieval_pgvector_required.sql` creates the extension, owns the
  `embedding_vector vector(768)` column, backfills it from the array, and
  drops the array; `0091` owns the HNSW index under the name the service
  used. The service verifies the schema at startup instead of creating it,
  the exact-scan search path and the dual-write upsert are gone, and the
  integration harness and e2e stack run `pgvector/pgvector:pg16`. The
  retrieval tests now embed at 768 dimensions. ADR 0003 records the
  reversal.

- **Vacuum result (2026-09-23 07:37–07:42 UTC).** `retrieval_embeddings`
  7,328 → 6,836 MB, `timeline_turns` 217 → 186 MB, `workspaces` 528 →
  440 kB, `repo_runtime_state` unchanged at 368 kB; database 25 → 24 GB.
  The embeddings table's TOAST was mostly live data (the vector stored
  twice), not dead space; the HNSW rebuild noted `maintenance_work_mem` is
  too small for the graph, which only slows the rebuild.
- **Trial durable dump.** `pg_dump -Fc` with the PostgreSQL 18 client
  against production, excluding the transcript-derived, retrieval, and code
  tables: 3.1 MB. Monthly dumps are negligible; keep every one.

Still not done: `git push` and `terraform apply` (both through the deploy
pipeline, on the user's say-so).

## Current state

Completed: exploration, measurement, plan revision 3, M1–M5 implementation
and tests. Remaining: M0's re-measure, trial dump, and vacuum; commit and
push; the first deploy (Terraform apply through the pipeline creates the
bucket and role policy). Next action: user reviews the diff, renews the
database grant for the re-measure, names a vacuum window, and authorizes
commit and push.
