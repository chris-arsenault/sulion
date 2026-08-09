-- One logical organization level above repositories. Collection membership
-- stays in Postgres; PTY sessions retain only the selected group identity.

CREATE TABLE meta_repos (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    primary_repo_name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX meta_repos_name_uidx
    ON meta_repos (LOWER(name));

CREATE TABLE meta_repo_members (
    meta_repo_id UUID NOT NULL REFERENCES meta_repos(id) ON DELETE CASCADE,
    repo_name TEXT NOT NULL UNIQUE,
    PRIMARY KEY (meta_repo_id, repo_name)
);

ALTER TABLE pty_sessions
    ADD COLUMN meta_repo_id UUID REFERENCES meta_repos(id) ON DELETE SET NULL;

CREATE INDEX pty_sessions_meta_repo_idx
    ON pty_sessions (meta_repo_id, created_at DESC)
    WHERE meta_repo_id IS NOT NULL AND state <> 'deleted';
