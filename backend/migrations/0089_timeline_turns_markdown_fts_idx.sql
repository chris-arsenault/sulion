-- no-transaction
-- Lexical search over archived sessions runs against the turn digest, the
-- only text a purged session keeps. Full-text rather than trigram: the
-- digest corpus is the whole history's markdown, and a GIN tsvector index is
-- a fraction of the trigram index's size for the same recall on word queries.
CREATE INDEX CONCURRENTLY IF NOT EXISTS timeline_turns_markdown_fts_idx
    ON timeline_turns USING gin (to_tsvector('simple', markdown))
    WHERE octet_length(markdown) <= 1000000;
