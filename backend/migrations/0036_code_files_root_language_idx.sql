-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS code_files_root_language_idx
    ON code_files(root_id, language)
    WHERE deleted_at IS NULL;
