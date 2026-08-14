-- no-transaction
-- Tool-name filters also constrain session and turn, which are served by the
-- operation primary key before applying the name predicate.
DROP INDEX CONCURRENTLY IF EXISTS timeline_operations_name_turn_idx;
