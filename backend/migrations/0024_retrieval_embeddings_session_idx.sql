-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS retrieval_embeddings_session_idx
    ON retrieval_embeddings(session_uuid, source_kind);
