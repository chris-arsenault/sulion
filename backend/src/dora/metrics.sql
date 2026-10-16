WITH repo_meta AS MATERIALIZED (
    SELECT repo_name AS repo, dora_checked_at AS checked_at,
        dora_scanned_since AS scanned_since, dora_error AS collection_error
    FROM repo_runtime_state WHERE exists = TRUE
), deployments AS MATERIALIZED (
    SELECT d.id, d.repo_name, d.deployed_at, d.outcome,
        d.failed_at, d.recovered_at, d.rework
    FROM dora_deployments d JOIN repo_meta r ON r.repo = d.repo_name
    WHERE d.deployed_at >= $1 AND d.deployed_at <= $2
), counts AS (
    SELECT COALESCE(repo_name, '') AS repo, COUNT(*) AS deployments,
        COUNT(*) FILTER (WHERE outcome <> 'unknown') AS classified_outcomes,
        COUNT(*) FILTER (WHERE outcome = 'failed') AS failed_deployments,
        COUNT(rework) AS classified_rework,
        COUNT(*) FILTER (WHERE rework) AS rework_count,
        COUNT(recovered_at) AS recovered_deployments,
        COUNT(*) FILTER (WHERE outcome = 'failed' AND recovered_at IS NULL) AS open_failures,
        percentile_cont(0.5) WITHIN GROUP (
            ORDER BY EXTRACT(EPOCH FROM (recovered_at - failed_at)) / 3600.0
        ) FILTER (WHERE recovered_at IS NOT NULL) AS recovery_time_hours_p50
    FROM deployments GROUP BY ROLLUP(repo_name)
), changes AS (
    SELECT COALESCE(d.repo_name, '') AS repo, COUNT(*) AS lead_time_samples,
        percentile_cont(0.5) WITHIN GROUP (
            ORDER BY EXTRACT(EPOCH FROM (d.deployed_at - c.committed_at)) / 3600.0
        ) AS change_lead_time_hours_p50
    FROM deployments d
    -- Keep commit lookup bounded by deployments in this window. Flattening
    -- this subquery can instead scan the entire historical commit projection.
    JOIN LATERAL (
        SELECT committed_at FROM dora_commit_deployments
        WHERE deployment_id = d.id AND NOT baseline AND committed_at <= d.deployed_at
        OFFSET 0
    ) c ON TRUE
    GROUP BY ROLLUP(d.repo_name)
), scopes AS (
    SELECT *, 1::bigint AS repository_count,
        (checked_at IS NOT NULL)::int::bigint AS collected_repository_count
    FROM repo_meta
    UNION ALL
    SELECT '', MIN(checked_at), MIN(scanned_since),
        CASE WHEN COUNT(collection_error) > 0
            THEN 'Repository collection errors: ' || COUNT(collection_error)::text END,
        COUNT(*), COUNT(checked_at)
    FROM repo_meta
)
SELECT s.*, COALESCE(n.deployments, 0) AS deployments,
    CASE WHEN s.collected_repository_count > 0
        THEN COALESCE(n.deployments, 0)::float8 / $3 END AS deployments_per_day,
    c.change_lead_time_hours_p50, COALESCE(c.lead_time_samples, 0) AS lead_time_samples,
    COALESCE(n.classified_outcomes, 0) AS classified_outcomes,
    COALESCE(n.failed_deployments, 0) AS failed_deployments,
    CASE WHEN n.deployments > 0 AND n.classified_outcomes = n.deployments
        THEN n.failed_deployments::float8 / n.deployments END AS change_failure_rate,
    n.recovery_time_hours_p50, COALESCE(n.recovered_deployments, 0) AS recovered_deployments,
    COALESCE(n.open_failures, 0) AS open_failures,
    COALESCE(n.classified_rework, 0) AS classified_rework,
    CASE WHEN n.deployments > 0 AND n.classified_rework = n.deployments
        THEN n.rework_count::float8 / n.deployments END AS rework_rate
FROM scopes s LEFT JOIN counts n USING (repo) LEFT JOIN changes c USING (repo)
ORDER BY s.repo;
