-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS timeline_operations_result_trgm_idx
    ON timeline_operations USING gin ((COALESCE(result_content, result_payload::text)) gin_trgm_ops)
    WHERE result_content IS NOT NULL OR result_payload IS NOT NULL;
