-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS code_references_root_name_idx
    ON code_references(root_id, referenced_name, start_line, start_col);
