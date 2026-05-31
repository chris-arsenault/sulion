-- pgvector is not a trusted extension, so ordinary app-role migrations
-- cannot install it. When a DBA has already run `CREATE EXTENSION vector`,
-- this migration adds the optional ANN-ready column for the local embedding
-- service. The retrieval service repeats this idempotent check at startup so
-- installing pgvector after this migration has run can still enable semantic
-- search.

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector') THEN
        EXECUTE 'ALTER TABLE retrieval_embeddings ADD COLUMN IF NOT EXISTS embedding_vector vector(768)';
    END IF;
END $$;
