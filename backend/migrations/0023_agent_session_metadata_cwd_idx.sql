-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS agent_session_metadata_cwd_idx
    ON agent_session_metadata(cwd);
