-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS code_roots_kind_name_idx
    ON code_roots(root_kind, name)
    WHERE deleted_at IS NULL;
