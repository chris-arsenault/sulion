-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS code_references_file_name_idx
    ON code_references(file_id, referenced_name);
