-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS code_imports_file_path_idx
    ON code_imports(file_id, import_path);
