-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS timeline_operations_name_turn_idx
    ON timeline_operations(name, session_uuid, turn_id);
