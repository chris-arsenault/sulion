-- Once a transcript supplies per-response usage, token_count only describes
-- context pressure. Its spend totals omit compaction responses.
ALTER TABLE agent_session_usage
    ADD COLUMN codex_response_records BOOLEAN NOT NULL DEFAULT FALSE;

-- A replayed response can appear at a new byte offset, including after other
-- responses. Keep response identity durable across ingester restarts.
CREATE TABLE agent_usage_responses (
    session_uuid UUID NOT NULL REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    response_id TEXT NOT NULL,
    PRIMARY KEY (session_uuid, response_id)
);
