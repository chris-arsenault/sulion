-- no-transaction
-- Production reads identify operations by their primary key. No query filters
-- operations by session and pair ID.
DROP INDEX CONCURRENTLY IF EXISTS timeline_operations_session_pair_idx;
