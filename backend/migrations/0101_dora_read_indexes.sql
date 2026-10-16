CREATE INDEX dora_deployments_time_page_idx
    ON dora_deployments(deployed_at DESC, id DESC) INCLUDE (repo_name);
CREATE INDEX dora_deployments_repo_page_idx
    ON dora_deployments(repo_name, deployed_at DESC, id DESC);
DROP INDEX dora_deployments_repo_time_idx;

CREATE INDEX dora_commits_deployment_time_idx
    ON dora_commit_deployments(deployment_id, committed_at) WHERE NOT baseline;
DROP INDEX dora_commits_deployment_idx;
