-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS code_symbols_file_kind_idx
    ON code_symbols(file_id, kind);
