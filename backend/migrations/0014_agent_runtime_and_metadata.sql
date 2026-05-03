-- First-class agent launch/runtime state plus transcript-derived UI metadata.
--
-- Runtime state is PTY-scoped: it answers "is an agent process currently
-- running in this terminal?" Metadata is agent-session-scoped: it answers
-- "what model/settings did this transcript report?"

ALTER TABLE pty_sessions
    ADD COLUMN agent_runtime_agent TEXT,
    ADD COLUMN agent_runtime_state TEXT NOT NULL DEFAULT 'none',
    ADD COLUMN agent_runtime_started_at TIMESTAMPTZ,
    ADD COLUMN agent_runtime_ended_at TIMESTAMPTZ,
    ADD COLUMN agent_runtime_exit_code INTEGER;

ALTER TABLE pty_sessions
    ADD CONSTRAINT pty_sessions_agent_runtime_state_check
    CHECK (agent_runtime_state IN ('none', 'starting', 'running', 'exited'));

CREATE INDEX pty_sessions_agent_runtime_idx
    ON pty_sessions(agent_runtime_state)
    WHERE agent_runtime_state IN ('starting', 'running');

CREATE TABLE agent_session_metadata (
    session_uuid UUID PRIMARY KEY REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    agent TEXT NOT NULL,
    model TEXT,
    model_provider TEXT,
    reasoning_effort TEXT,
    cli_version TEXT,
    cwd TEXT,
    model_context_window BIGINT,
    raw_metadata_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
