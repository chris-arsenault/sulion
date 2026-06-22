-- Chunked embeddings: one source can now map to multiple embedding rows
-- (chunk_ord 0..N-1) instead of a single truncated row, so `retrieval_embeddings`
-- is keyed by (embedding_model, source_key, chunk_ord). `retrieval_embedding_sources`
-- stays one-row-per-source, so index status and source counts remain source-based.
--
-- This is a NON-DESTRUCTIVE schema change. Existing rows get chunk_ord = 0, which
-- preserves uniqueness (the old (embedding_model, source_key) key had no
-- duplicates, so (embedding_model, source_key, 0) has none either). Existing
-- embeddings remain valid and searchable; they are replaced incrementally as
-- their sources are re-indexed.
--
-- Re-applying every source under the new filtering and chunking rules is a
-- separate, MANUALLY run operation (scripts/retrieval-reset-index.sql) and is
-- deliberately NOT tied to deploy -- migrations must never carry a destructive
-- data wipe.

ALTER TABLE retrieval_embeddings
    ADD COLUMN IF NOT EXISTS chunk_ord INT NOT NULL DEFAULT 0;

ALTER TABLE retrieval_embeddings
    DROP CONSTRAINT IF EXISTS retrieval_embeddings_embedding_model_source_key_key;

ALTER TABLE retrieval_embeddings
    ADD CONSTRAINT retrieval_embeddings_model_source_chunk_key
    UNIQUE (embedding_model, source_key, chunk_ord);
