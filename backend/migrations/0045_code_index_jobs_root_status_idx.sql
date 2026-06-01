-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS code_index_jobs_root_status_idx
    ON code_index_jobs(root_id, status, created_at DESC);
