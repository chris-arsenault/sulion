-- Sulion-managed workspaces bind PTY sessions to either the canonical
-- repo checkout or an isolated git worktree branch.

CREATE TABLE workspaces (
    id UUID PRIMARY KEY,
    repo_name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('main', 'worktree')),
    path TEXT NOT NULL UNIQUE,
    branch_name TEXT,
    base_ref TEXT,
    base_sha TEXT,
    merge_target TEXT,
    created_by_session_id UUID,
    state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'missing', 'deleted')),
    git_revision BIGINT NOT NULL DEFAULT 0,
    head_sha TEXT,
    head_subject TEXT,
    head_committed_at TIMESTAMPTZ,
    recent_commits_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    dirty_count INT NOT NULL DEFAULT 0,
    untracked_count INT NOT NULL DEFAULT 0,
    dirty_fingerprint TEXT NOT NULL DEFAULT '',
    status_started_at TIMESTAMPTZ,
    status_finished_at TIMESTAMPTZ,
    next_status_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    status_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX workspaces_main_repo_idx
    ON workspaces(repo_name)
    WHERE kind = 'main' AND state <> 'deleted';

CREATE INDEX workspaces_repo_idx
    ON workspaces(repo_name)
    WHERE state <> 'deleted';

CREATE INDEX workspaces_due_idx
    ON workspaces(next_status_at)
    WHERE state = 'active';

CREATE TABLE workspace_dirty_paths (
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    status TEXT NOT NULL,
    additions INT,
    deletions INT,
    PRIMARY KEY (workspace_id, path)
);

ALTER TABLE pty_sessions
    ADD COLUMN workspace_id UUID REFERENCES workspaces(id) ON DELETE SET NULL;

CREATE INDEX pty_sessions_workspace_idx
    ON pty_sessions(workspace_id)
    WHERE workspace_id IS NOT NULL;
