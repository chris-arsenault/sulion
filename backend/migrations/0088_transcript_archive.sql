-- Transcript archive: export idle sessions to object storage, roll cost and
-- file activity up to daily repo-level tables, and purge each exported session
-- down to its turn digest (docs/plans/transcript-archive-and-purge.md).
--
-- Nothing here deletes or rewrites data. The columns below are written by the
-- archive loop in the control process; every rebuild path checks `purged_at`
-- before it touches a session, because a purged session has no `events` rows
-- to rebuild from.

ALTER TABLE claude_sessions
    ADD COLUMN archived_at TIMESTAMPTZ,
    ADD COLUMN archive_key TEXT,
    ADD COLUMN archive_sha256 TEXT,
    ADD COLUMN archive_bytes BIGINT,
    ADD COLUMN archive_events BIGINT,
    ADD COLUMN purged_at TIMESTAMPTZ;

CREATE INDEX claude_sessions_archived_idx
    ON claude_sessions(archived_at)
    WHERE archived_at IS NOT NULL;

CREATE INDEX claude_sessions_purged_idx
    ON claude_sessions(purged_at)
    WHERE purged_at IS NOT NULL;

-- A purged session keeps its `events` rows only until the export is verified;
-- afterwards they are deleted outright. The column stays NOT NULL for live
-- rows in practice, but a restore in progress inserts through the normal
-- ingest path, which always supplies a payload, so the constraint could stay.
-- It is relaxed anyway so a future partial purge (payload gone, row kept)
-- needs no further migration.
ALTER TABLE events ALTER COLUMN payload DROP NOT NULL;

-- On-demand work for the archive loop: a cycle run or a restore, requested
-- from the CLI over the correlate socket or from the admin API, picked up by
-- the control process. `scope` is the restore selector (session, month, repo,
-- or all) and `result_json` is what the loop reports back.
CREATE TABLE archive_requests (
    id BIGSERIAL PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('run', 'restore', 'verify')),
    scope JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'completed', 'failed')),
    requested_by TEXT,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    error TEXT,
    result_json JSONB,
    job_id BIGINT
);

CREATE INDEX archive_requests_pending_idx
    ON archive_requests(requested_at)
    WHERE status = 'pending';

-- When the last full cycle ran. One row; the loop compares against the
-- configured interval rather than reading job history, which is pruned.
--
-- `purge_enabled` is the operator's gate: until it is set, a cycle exports
-- and dumps but deletes nothing, so the first backup can be verified with
-- `sulion archive verify --deep` before any row is removed.
CREATE TABLE archive_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_cycle_started_at TIMESTAMPTZ,
    last_cycle_completed_at TIMESTAMPTZ,
    last_dump_key TEXT,
    last_dump_at TIMESTAMPTZ,
    purge_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    purge_enabled_at TIMESTAMPTZ
);

INSERT INTO archive_state (id) VALUES (1);

-- Token usage rolled up to the dimensions the cost report is built from. Rows
-- are added when a session is purged, with the same repo attribution the
-- metrics query applies to live sessions at that moment, so the report is the
-- union of this table and the per-session daily rows that still exist.
CREATE TABLE usage_daily_rollup (
    day DATE NOT NULL,
    repo TEXT NOT NULL DEFAULT '',
    agent TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    cached_input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (cached_input_tokens >= 0),
    cache_write_input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (cache_write_input_tokens >= 0),
    cache_write_1h_input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (cache_write_1h_input_tokens >= 0),
    output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (day, repo, agent, model)
);

-- What each purged session added to the rollup, so a restore subtracts
-- exactly that before the replay recomputes the session's own rows.
CREATE TABLE usage_rollup_contributions (
    session_uuid UUID NOT NULL REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    day DATE NOT NULL,
    repo TEXT NOT NULL DEFAULT '',
    agent TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0,
    cached_input_tokens BIGINT NOT NULL DEFAULT 0,
    cache_write_input_tokens BIGINT NOT NULL DEFAULT 0,
    cache_write_1h_input_tokens BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (session_uuid, day, repo, agent, model)
);

-- File churn rolled up per repo, path, and day: how many turns wrote or read
-- the path. The hotspot query keeps reading live touches over its 7-day
-- window; this is the durable series behind it once touches are purged.
CREATE TABLE file_activity_daily (
    repo TEXT NOT NULL,
    path TEXT NOT NULL,
    day DATE NOT NULL,
    write_turns BIGINT NOT NULL DEFAULT 0 CHECK (write_turns >= 0),
    read_turns BIGINT NOT NULL DEFAULT 0 CHECK (read_turns >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (repo, path, day)
);

CREATE INDEX file_activity_daily_day_idx ON file_activity_daily(day);

CREATE TABLE file_activity_contributions (
    session_uuid UUID NOT NULL REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    repo TEXT NOT NULL,
    path TEXT NOT NULL,
    day DATE NOT NULL,
    write_turns BIGINT NOT NULL DEFAULT 0,
    read_turns BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (session_uuid, repo, path, day)
);

-- The turn digest carries the files a turn touched once the per-touch rows
-- are gone: `[{"repo": ..., "path": ..., "kind": ..., "write": bool}]`.
-- Empty for live turns; file-history and evidence read it only for purged
-- sessions.
ALTER TABLE timeline_turns
    ADD COLUMN files_json JSONB NOT NULL DEFAULT '[]'::jsonb;

CREATE INDEX timeline_turns_files_json_idx
    ON timeline_turns USING gin (files_json jsonb_path_ops);

-- Semantic search over purged sessions embeds the turn digest itself. The
-- source kind already exists in the enum; the family did not.
ALTER TABLE retrieval_embedding_sources
    DROP CONSTRAINT IF EXISTS retrieval_embedding_sources_source_family_check;

ALTER TABLE retrieval_embedding_sources
    ADD CONSTRAINT retrieval_embedding_sources_source_family_check
    CHECK (source_family IN ('event_block', 'operation_call', 'operation_result', 'turn_digest'));

ALTER TABLE retrieval_embedding_backfills
    DROP CONSTRAINT IF EXISTS retrieval_embedding_backfills_source_family_check;

ALTER TABLE retrieval_embedding_backfills
    ADD CONSTRAINT retrieval_embedding_backfills_source_family_check
    CHECK (source_family IN ('event_block', 'operation_call', 'operation_result', 'turn_digest'));

-- These three tables are rewritten far more often than their row counts
-- suggest (a turn is re-upserted whole on every appended event; status JSON
-- is replaced per poll), and the default 20% dead-tuple threshold let them
-- carry gigabytes of dead space. Vacuum them at 2%.
ALTER TABLE timeline_turns SET (autovacuum_vacuum_scale_factor = 0.02);
ALTER TABLE retrieval_embeddings SET (autovacuum_vacuum_scale_factor = 0.02);
ALTER TABLE repo_runtime_state SET (autovacuum_vacuum_scale_factor = 0.02, autovacuum_vacuum_threshold = 20);
