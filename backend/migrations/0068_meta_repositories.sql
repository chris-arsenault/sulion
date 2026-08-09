-- One metadata-only organization level above repositories, plus an immutable
-- repository/workspace snapshot for each PTY session.

CREATE TABLE meta_repos (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    primary_repo_name TEXT,
    position INTEGER NOT NULL DEFAULT 0,
    revision BIGINT NOT NULL DEFAULT 1,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX meta_repos_name_active_uidx
    ON meta_repos (LOWER(name))
    WHERE deleted_at IS NULL;

CREATE INDEX meta_repos_active_position_idx
    ON meta_repos (position, name)
    WHERE deleted_at IS NULL;

CREATE TABLE meta_repo_members (
    meta_repo_id UUID NOT NULL REFERENCES meta_repos(id) ON DELETE CASCADE,
    repo_name TEXT NOT NULL UNIQUE,
    position INTEGER NOT NULL,
    PRIMARY KEY (meta_repo_id, repo_name),
    UNIQUE (meta_repo_id, position)
);

ALTER TABLE pty_sessions
    ADD COLUMN meta_repo_id UUID REFERENCES meta_repos(id) ON DELETE SET NULL,
    ADD COLUMN meta_repo_name TEXT;

CREATE INDEX pty_sessions_meta_repo_idx
    ON pty_sessions (meta_repo_id, created_at DESC)
    WHERE meta_repo_id IS NOT NULL AND state <> 'deleted';

CREATE TABLE pty_session_repos (
    pty_session_id UUID NOT NULL REFERENCES pty_sessions(id) ON DELETE CASCADE,
    repo_name TEXT NOT NULL,
    workspace_id UUID REFERENCES workspaces(id) ON DELETE SET NULL,
    role TEXT NOT NULL CHECK (role IN ('primary', 'additional')),
    position INTEGER NOT NULL,
    PRIMARY KEY (pty_session_id, repo_name),
    UNIQUE (pty_session_id, position)
);

CREATE UNIQUE INDEX pty_session_repos_primary_uidx
    ON pty_session_repos (pty_session_id)
    WHERE role = 'primary';

CREATE INDEX pty_session_repos_repo_idx
    ON pty_session_repos (repo_name, pty_session_id);

CREATE INDEX pty_session_repos_workspace_idx
    ON pty_session_repos (workspace_id, pty_session_id)
    WHERE workspace_id IS NOT NULL;

INSERT INTO pty_session_repos (
    pty_session_id,
    repo_name,
    workspace_id,
    role,
    position
)
SELECT id, repo, workspace_id, 'primary', 0
  FROM pty_sessions
ON CONFLICT (pty_session_id, repo_name) DO NOTHING;
