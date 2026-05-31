-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS timeline_operations_input_trgm_idx
    ON timeline_operations USING gin ((input::text) gin_trgm_ops)
    WHERE input IS NOT NULL;
