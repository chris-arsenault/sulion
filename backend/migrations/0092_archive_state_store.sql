-- Which store the control process's archive loop is writing to, recorded by
-- the loop itself when it starts. `sulion archive status` runs on the node
-- over the correlate socket, where the control plane's environment is not
-- visible, so it reads this rather than guessing from its own.
ALTER TABLE archive_state
    ADD COLUMN store TEXT,
    ADD COLUMN loop_started_at TIMESTAMPTZ;
