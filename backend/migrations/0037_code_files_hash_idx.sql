-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS code_files_hash_idx
    ON code_files(content_hash)
    WHERE content_hash IS NOT NULL AND deleted_at IS NULL;
