-- no-transaction
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS code_roots_active_path_uidx
    ON code_roots(path)
    WHERE deleted_at IS NULL;
