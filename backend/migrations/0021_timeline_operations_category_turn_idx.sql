-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS timeline_operations_category_turn_idx
    ON timeline_operations(operation_category, session_uuid, turn_id);
