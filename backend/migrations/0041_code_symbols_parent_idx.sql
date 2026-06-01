-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS code_symbols_parent_idx
    ON code_symbols(parent_symbol_id)
    WHERE parent_symbol_id IS NOT NULL;
