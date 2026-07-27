-- Agent-chosen terminal name, set via `sulion name` over the control
-- socket. Complements (never replaces) the user's `label`: surfaced
-- beside it in the sidebar and monitor, deliberately absent from tab
-- headers.
ALTER TABLE pty_sessions
    ADD COLUMN agent_label TEXT;
