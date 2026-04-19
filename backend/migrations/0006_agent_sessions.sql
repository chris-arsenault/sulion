-- Generalise Claude-specific session pointers so other transcript
-- sources can coexist. We keep the legacy `claude_sessions` table name
-- for compatibility, but rows now carry an `agent` discriminator and
-- PTY rows point at a generic current session.

ALTER TABLE claude_sessions
    ADD COLUMN agent TEXT NOT NULL DEFAULT 'claude-code';

CREATE INDEX claude_sessions_agent_idx ON claude_sessions(agent);

ALTER TABLE pty_sessions
    ADD COLUMN current_session_uuid UUID,
    ADD COLUMN current_session_agent TEXT;

UPDATE pty_sessions
   SET current_session_uuid = current_claude_session_uuid,
       current_session_agent = CASE
           WHEN current_claude_session_uuid IS NULL THEN NULL
           ELSE 'claude-code'
       END
 WHERE current_claude_session_uuid IS NOT NULL;

CREATE INDEX pty_sessions_current_session_uuid_idx
    ON pty_sessions(current_session_uuid)
    WHERE current_session_uuid IS NOT NULL;
