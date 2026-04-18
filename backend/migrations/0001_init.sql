-- PTY sessions are the primary session object managed by the backend.
-- A PTY session may host zero, one, or many Claude sessions over its lifetime.
CREATE TABLE pty_sessions (
    id UUID PRIMARY KEY,
    repo TEXT NOT NULL,
    working_dir TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('live', 'dead', 'deleted')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at TIMESTAMPTZ,
    exit_code INTEGER,
    -- Pointer to the most recent Claude session started in this PTY.
    -- Nullable: a PTY exists before any `claude` is invoked.
    -- Not a FK because ingester and correlation can race.
    current_claude_session_uuid UUID
);

CREATE INDEX pty_sessions_state_idx ON pty_sessions(state);
CREATE INDEX pty_sessions_created_idx ON pty_sessions(created_at DESC);

-- Claude sessions correspond 1:1 with JSONL files under ~/.claude/projects/.
-- Rows are created by either the ingester (first event seen) or the
-- SessionStart hook (correlation arrives) — both paths use upserts.
CREATE TABLE claude_sessions (
    session_uuid UUID PRIMARY KEY,
    pty_session_id UUID,
    -- When Claude compacts a session, the new session references its predecessor.
    -- Populated by the ingester when it detects a compaction-continuation event.
    parent_session_uuid UUID,
    project_hash TEXT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX claude_sessions_pty_idx ON claude_sessions(pty_session_id);
CREATE INDEX claude_sessions_parent_idx ON claude_sessions(parent_session_uuid);

-- Timeline events ingested from JSONL. Byte offset is load-bearing for
-- idempotency: JSONL files are append-only, so offset uniquely identifies
-- a line. ON CONFLICT DO NOTHING lets the ingester replay safely.
CREATE TABLE events (
    session_uuid UUID NOT NULL REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    byte_offset BIGINT NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    kind TEXT NOT NULL,
    payload JSONB NOT NULL,
    PRIMARY KEY (session_uuid, byte_offset)
);

CREATE INDEX events_session_ts_idx ON events(session_uuid, timestamp);
CREATE INDEX events_kind_idx ON events(kind);

-- Per-session-file ingester offset. Lets the ingester resume on restart
-- without rereading already-committed bytes.
CREATE TABLE ingester_state (
    session_uuid UUID PRIMARY KEY,
    file_path TEXT NOT NULL,
    last_committed_byte_offset BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Registered repos the user can spawn PTY sessions into.
CREATE TABLE repos (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT UNIQUE NOT NULL,
    path TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
