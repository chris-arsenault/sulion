-- Materialized runtime state consumed by the unified app-state poll.
-- Request handlers read these rows; background/backend producers own
-- reconciliation and revision bumps.

CREATE TABLE IF NOT EXISTS repo_runtime_state (
    repo_name TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    exists BOOLEAN NOT NULL DEFAULT TRUE,
    git_revision BIGINT NOT NULL DEFAULT 0,
    branch TEXT,
    head_sha TEXT,
    head_subject TEXT,
    head_committed_at TIMESTAMPTZ,
    recent_commits_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    dirty_count INT NOT NULL DEFAULT 0,
    untracked_count INT NOT NULL DEFAULT 0,
    dirty_fingerprint TEXT NOT NULL DEFAULT '',
    status_started_at TIMESTAMPTZ,
    status_finished_at TIMESTAMPTZ,
    next_status_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    status_error TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS repo_runtime_state_due_idx
    ON repo_runtime_state(next_status_at)
    WHERE exists = TRUE;

CREATE TABLE IF NOT EXISTS repo_dirty_paths (
    repo_name TEXT NOT NULL REFERENCES repo_runtime_state(repo_name) ON DELETE CASCADE,
    path TEXT NOT NULL,
    status TEXT NOT NULL,
    additions INT,
    deletions INT,
    PRIMARY KEY (repo_name, path)
);

CREATE TABLE IF NOT EXISTS timeline_session_state (
    session_uuid UUID PRIMARY KEY REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    revision BIGINT NOT NULL DEFAULT 0,
    total_event_count BIGINT NOT NULL DEFAULT 0,
    turn_count BIGINT NOT NULL DEFAULT 0,
    latest_turn_id BIGINT,
    latest_event_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO timeline_session_state (
    session_uuid,
    revision,
    total_event_count,
    turn_count,
    latest_turn_id,
    latest_event_at,
    updated_at
)
SELECT
    cs.session_uuid,
    1,
    COALESCE(SUM(tt.event_count), 0)::BIGINT,
    COUNT(tt.turn_id)::BIGINT,
    MAX(tt.turn_id),
    MAX(tt.end_timestamp),
    NOW()
FROM claude_sessions cs
LEFT JOIN timeline_turns tt ON tt.session_uuid = cs.session_uuid
GROUP BY cs.session_uuid
ON CONFLICT (session_uuid) DO NOTHING;

CREATE TABLE IF NOT EXISTS future_prompt_session_state (
    session_uuid UUID PRIMARY KEY,
    revision BIGINT NOT NULL DEFAULT 0,
    pending_count INT NOT NULL DEFAULT 0,
    reconciled_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
