INSERT INTO repo_runtime_state(repo_name, path, exists, dora_checked_at, dora_scanned_since)
SELECT 'scale-' || r, '/scale/' || r, TRUE, NOW(), NOW() - INTERVAL '30 days'
FROM generate_series(1, 50) r;

INSERT INTO dora_deployments(id, repo_name, source_key, revision, deployed_at, source_url, outcome, rework)
SELECT md5(r || ':recent:' || n)::uuid, 'scale-' || r, 'recent-' || n,
    lpad(n::text, 40, '0'), NOW() - n * INTERVAL '1 hour',
    'https://github.com/scale/app/actions/runs/' || n, 'healthy', FALSE
FROM generate_series(1, 50) r CROSS JOIN generate_series(1, 200) n;

INSERT INTO dora_deployments(id, repo_name, source_key, revision, deployed_at, source_url, outcome, rework)
SELECT md5(r || ':old:' || n)::uuid, 'scale-' || r, 'old-' || n,
    lpad((n + 200)::text, 40, '0'), NOW() - INTERVAL '365 days' - n * INTERVAL '1 hour',
    'https://github.com/scale/app/actions/runs/' || (n + 200), 'healthy', FALSE
FROM generate_series(1, 50) r CROSS JOIN generate_series(1, 1000) n;

INSERT INTO dora_commit_deployments(repo_name, revision, deployment_id, committed_at, baseline)
SELECT d.repo_name, md5(d.id::text || ':' || c), d.id,
    d.deployed_at - INTERVAL '2 hours', FALSE
FROM dora_deployments d CROSS JOIN generate_series(1, 5) c;

ANALYZE repo_runtime_state;
ANALYZE dora_deployments;
ANALYZE dora_commit_deployments;
