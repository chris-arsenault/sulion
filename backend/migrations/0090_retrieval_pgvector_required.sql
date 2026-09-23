-- pgvector is required. The `embedding REAL[]` column was the pre-pgvector
-- store and the exact-scan fallback's input; every deployment has held the
-- same vector twice since the `embedding_vector` column was added lazily by
-- the retrieval service at startup. Own that column here, fill it for any
-- row that still lacks it, and drop the array.
--
-- On the TrueNAS instance the extension, column, and index already exist,
-- so the statements below are no-ops apart from the drop. The dropped
-- column's bytes are reclaimed as rows are rewritten or by a VACUUM FULL of
-- retrieval_embeddings, which cannot run inside this transaction.

CREATE EXTENSION IF NOT EXISTS vector;

ALTER TABLE retrieval_embeddings
    ADD COLUMN IF NOT EXISTS embedding_vector vector(768);

UPDATE retrieval_embeddings
   SET embedding_vector = embedding::vector
 WHERE embedding_vector IS NULL
   AND cardinality(embedding) = 768;

-- A row whose array is not 768 wide never matched the configured model and
-- was unreachable by search; the indexer re-embeds its source when asked.
DELETE FROM retrieval_embeddings WHERE embedding_vector IS NULL;

ALTER TABLE retrieval_embeddings
    ALTER COLUMN embedding_vector SET NOT NULL;

ALTER TABLE retrieval_embeddings DROP COLUMN embedding;
