-- no-transaction
-- Retrieval search combines operation fields into a ranked document; it does
-- not issue predicates that can use this expression index.
DROP INDEX CONCURRENTLY IF EXISTS timeline_operations_result_trgm_idx;
