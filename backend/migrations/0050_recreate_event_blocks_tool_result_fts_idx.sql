-- no-transaction
CREATE INDEX CONCURRENTLY event_blocks_tool_result_fts_idx
    ON event_blocks USING gin (to_tsvector('simple', text))
    WHERE kind = 'tool_result'
      AND text IS NOT NULL
      AND octet_length(text) <= 1000000;
