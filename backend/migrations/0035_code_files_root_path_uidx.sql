-- no-transaction
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS code_files_root_path_uidx
    ON code_files(root_id, path);
