-- Which devenv hosts each session: the container image ID for versioned
-- devenvs, 'default' for child-mode hosting, NULL for rows that predate
-- versioning or never had a process. Additive and nullable on purpose —
-- control and node release independently.
ALTER TABLE pty_sessions
    ADD COLUMN IF NOT EXISTS devenv_ident TEXT;
