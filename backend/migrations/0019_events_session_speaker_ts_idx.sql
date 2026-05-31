-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS events_session_speaker_ts_idx
    ON events(session_uuid, speaker, timestamp DESC);
