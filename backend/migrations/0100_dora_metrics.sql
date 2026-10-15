CREATE TABLE dora_deployments (
    id UUID PRIMARY KEY,
    repo_name TEXT NOT NULL,
    source_key TEXT NOT NULL,
    revision TEXT NOT NULL,
    deployed_at TIMESTAMPTZ NOT NULL,
    source_url TEXT NOT NULL,
    commits_collected BOOLEAN NOT NULL DEFAULT FALSE,
    outcome TEXT NOT NULL DEFAULT 'unknown' CHECK (outcome IN ('unknown', 'healthy', 'failed')),
    failed_at TIMESTAMPTZ,
    recovered_at TIMESTAMPTZ,
    rework BOOLEAN,
    UNIQUE (repo_name, source_key),
    CHECK ((outcome = 'failed') = (failed_at IS NOT NULL)),
    CHECK (failed_at IS NULL OR failed_at >= deployed_at),
    CHECK (recovered_at IS NULL OR (failed_at IS NOT NULL AND recovered_at >= failed_at))
);
CREATE INDEX dora_deployments_repo_time_idx ON dora_deployments(repo_name, deployed_at);

CREATE TABLE dora_commit_deployments (
    repo_name TEXT NOT NULL,
    revision TEXT NOT NULL,
    deployment_id UUID NOT NULL REFERENCES dora_deployments(id) ON DELETE CASCADE,
    committed_at TIMESTAMPTZ NOT NULL,
    baseline BOOLEAN NOT NULL,
    PRIMARY KEY (repo_name, revision)
);
CREATE INDEX dora_commits_deployment_idx ON dora_commit_deployments(deployment_id) WHERE NOT baseline;

CREATE TABLE dora_run_checks (
    repo_name TEXT NOT NULL REFERENCES repo_runtime_state(repo_name) ON UPDATE CASCADE ON DELETE CASCADE,
    source_key TEXT NOT NULL,
    production_source BOOLEAN NOT NULL,
    PRIMARY KEY (repo_name, source_key)
);

ALTER TABLE repo_runtime_state
    ADD COLUMN dora_checked_at TIMESTAMPTZ,
    ADD COLUMN dora_scanned_since TIMESTAMPTZ,
    ADD COLUMN dora_error TEXT,
    ADD COLUMN next_dora_at TIMESTAMPTZ NOT NULL DEFAULT NOW();
