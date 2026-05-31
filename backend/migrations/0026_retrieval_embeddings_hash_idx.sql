-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS retrieval_embeddings_hash_idx
    ON retrieval_embeddings(embedding_model, content_hash);
