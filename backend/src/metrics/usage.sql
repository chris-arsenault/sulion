WITH RECURSIVE repo_hashes AS MATERIALIZED (
    SELECT repo_name, regexp_replace(path, '[^A-Za-z0-9]', '-', 'g') AS project_hash
    FROM repo_runtime_state
 ), lineage AS (
    SELECT cs.session_uuid AS origin, cs.pty_session_id,
           cs.parent_session_uuid, 0 AS depth
    FROM claude_sessions cs
  UNION ALL
    SELECT l.origin, parent.pty_session_id, parent.parent_session_uuid,
           l.depth + 1
    FROM lineage l
    JOIN claude_sessions parent ON parent.session_uuid = l.parent_session_uuid
    WHERE l.pty_session_id IS NULL AND l.depth < 16
 ), lineage_pty AS (
    SELECT DISTINCT ON (origin) origin, pty_session_id
    FROM lineage WHERE pty_session_id IS NOT NULL ORDER BY origin, depth
 ), dimensions AS (
    SELECT cs.session_uuid, metadata.model,
        COALESCE(p_direct.repo, p_reverse.repo, hash_repo.repo_name) AS repo
    FROM claude_sessions cs
    LEFT JOIN agent_session_metadata metadata ON metadata.session_uuid = cs.session_uuid
    LEFT JOIN lineage_pty lp ON lp.origin = cs.session_uuid
    LEFT JOIN pty_sessions p_direct ON p_direct.id = lp.pty_session_id
    LEFT JOIN LATERAL (
        SELECT pr.repo FROM pty_sessions pr
        WHERE pr.current_session_uuid = cs.session_uuid LIMIT 1
    ) p_reverse ON TRUE
    LEFT JOIN LATERAL (
        SELECT r.repo_name FROM repo_hashes r
        WHERE cs.project_hash IS NOT NULL
          AND r.project_hash = cs.project_hash
        LIMIT 1
    ) hash_repo ON TRUE
 ), live AS (
    SELECT d.day, dimensions.repo, d.agent, d.model,
        d.input_tokens, d.cached_input_tokens, d.cache_write_input_tokens,
        d.cache_write_1h_input_tokens, d.output_tokens
    FROM agent_model_usage_daily d
    LEFT JOIN dimensions ON dimensions.session_uuid = d.session_uuid
 ), archived AS (
    -- Purged sessions: their per-session rows were rolled up at purge time
    -- with the attribution above frozen in, so the report is the union.
    SELECT r.day, NULLIF(r.repo, '') AS repo, r.agent, r.model,
        r.input_tokens, r.cached_input_tokens, r.cache_write_input_tokens,
        r.cache_write_1h_input_tokens, r.output_tokens
    FROM usage_daily_rollup r
 ), combined AS (
    SELECT * FROM live UNION ALL SELECT * FROM archived
 )
 SELECT c.day, c.repo, c.agent, c.model,
    COALESCE(SUM(c.input_tokens), 0)::BIGINT AS standard_input,
    COALESCE(SUM(c.cached_input_tokens), 0)::BIGINT AS cache_read,
    COALESCE(SUM(c.cache_write_input_tokens), 0)::BIGINT AS cache_write,
    COALESCE(SUM(c.cache_write_1h_input_tokens), 0)::BIGINT AS cache_write_1h,
    COALESCE(SUM(c.output_tokens), 0)::BIGINT AS output
 FROM combined c
 GROUP BY c.day, c.repo, c.agent, c.model
 ORDER BY c.day;
