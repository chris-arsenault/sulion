-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS timeline_turns_session_time_idx
    ON timeline_turns(session_uuid, start_timestamp, end_timestamp);
