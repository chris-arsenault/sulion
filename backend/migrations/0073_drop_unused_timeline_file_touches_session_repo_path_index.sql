-- no-transaction
-- File history reads filter by repository and path across sessions, while
-- session-scoped reads identify touches by session and turn.
DROP INDEX CONCURRENTLY IF EXISTS timeline_file_touches_session_repo_path_idx;
