-- Add an 'orphaned' PTY state for rows that were 'live' at the moment
-- the backend restarted. Those shell processes are gone, but we never
-- got a supervisor signal to mark them dead. Reconciliation at startup
-- transitions all 'live' rows to 'orphaned' with ended_at = NOW().

ALTER TABLE pty_sessions DROP CONSTRAINT pty_sessions_state_check;
ALTER TABLE pty_sessions ADD CONSTRAINT pty_sessions_state_check
    CHECK (state IN ('live', 'dead', 'deleted', 'orphaned'));
