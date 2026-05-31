-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS retrieval_embeddings_repo_idx
    ON retrieval_embeddings(repo_name, source_kind)
    WHERE repo_name IS NOT NULL;
