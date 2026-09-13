-- Prompts submitted from the timeline input, recorded before their bytes
-- reach the PTY. A harness sitting on a startup dialog swallows typed text
-- without leaving a transcript trace, so the transcript cannot be the record
-- of what the user asked. A row is matched once the ingester projects a
-- timeline turn with the same text in one of the PTY's transcript sessions;
-- until then it is the only copy.
--
-- The correlation timestamp tells a session reported for this launch apart
-- from one left over from the previous agent in the same PTY: a running
-- harness whose correlation predates its own start is still on its startup
-- screens. Existing rows are stamped with their launch time so live sessions
-- do not read as uncorrelated after the upgrade.

ALTER TABLE pty_sessions ADD COLUMN current_session_correlated_at TIMESTAMPTZ;

UPDATE pty_sessions
   SET current_session_correlated_at = COALESCE(agent_runtime_started_at, NOW())
 WHERE current_session_uuid IS NOT NULL;

CREATE TABLE submitted_prompts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    pty_session_id UUID NOT NULL REFERENCES pty_sessions(id) ON DELETE CASCADE,
    agent TEXT,
    text TEXT NOT NULL,
    forced BOOLEAN NOT NULL DEFAULT FALSE,
    submitted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    delivery_error TEXT,
    matched_session_uuid UUID,
    matched_turn_id BIGINT,
    matched_at TIMESTAMPTZ,
    dismissed_at TIMESTAMPTZ
);

CREATE INDEX submitted_prompts_pty_recent_idx
    ON submitted_prompts (pty_session_id, submitted_at DESC);

CREATE INDEX submitted_prompts_open_idx
    ON submitted_prompts (pty_session_id)
    WHERE matched_at IS NULL AND dismissed_at IS NULL;
