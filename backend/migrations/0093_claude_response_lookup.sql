-- Streamed blocks revise one response's usage. Retained source events are
-- the durable response ledger used to subtract the previously applied value.
CREATE INDEX events_claude_response_idx
    ON events (session_uuid, (payload #>> '{message,id}'), byte_offset DESC)
    WHERE agent = 'claude-code' AND kind = 'assistant';
