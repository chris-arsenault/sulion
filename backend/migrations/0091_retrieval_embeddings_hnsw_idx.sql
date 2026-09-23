-- no-transaction
-- The ANN index the retrieval service used to create at startup when it
-- found it missing. Named and predicated exactly as before, so the index the
-- production instance already carries satisfies IF NOT EXISTS.
CREATE INDEX CONCURRENTLY IF NOT EXISTS retrieval_embeddings_embedding_hnsw_768_idx
    ON retrieval_embeddings USING hnsw (embedding_vector vector_cosine_ops)
    WHERE embedding_dimensions = 768;
