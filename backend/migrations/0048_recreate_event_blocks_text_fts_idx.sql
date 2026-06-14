-- no-transaction
CREATE INDEX CONCURRENTLY event_blocks_text_fts_idx
    ON event_blocks USING gin (to_tsvector('simple', text))
    WHERE kind = 'text'
      AND text IS NOT NULL
      AND octet_length(text) <= 1000000;
