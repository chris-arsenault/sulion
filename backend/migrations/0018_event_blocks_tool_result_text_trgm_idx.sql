-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS event_blocks_tool_result_text_trgm_idx
    ON event_blocks USING gin (text gin_trgm_ops)
    WHERE kind = 'tool_result' AND text IS NOT NULL;
