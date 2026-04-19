-- Ticket #21. User-facing metadata on PTY sessions: a human-readable
-- label (overrides the uuid in the sidebar), a pinned flag (float to
-- top of the repo group), and a palette-constrained colour tag.
--
-- All additive. Null label means "no override, fall back to the uuid
-- prefix." Null colour means "default tone per state dot."

ALTER TABLE pty_sessions
    ADD COLUMN label TEXT,
    ADD COLUMN pinned BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN color TEXT;

CREATE INDEX pty_sessions_pinned_idx ON pty_sessions(pinned) WHERE pinned = true;
