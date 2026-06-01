-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS code_symbols_root_name_idx
    ON code_symbols(root_id, name);
