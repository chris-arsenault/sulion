-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS event_blocks_text_trgm_idx
    ON event_blocks USING gin (text gin_trgm_ops)
    WHERE kind = 'text' AND text IS NOT NULL;
