-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS code_references_symbol_idx
    ON code_references(symbol_id)
    WHERE symbol_id IS NOT NULL;
