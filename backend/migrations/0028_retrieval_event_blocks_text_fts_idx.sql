-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS event_blocks_text_fts_idx
    ON event_blocks USING gin (to_tsvector('simple', text))
    WHERE kind = 'text' AND text IS NOT NULL;
