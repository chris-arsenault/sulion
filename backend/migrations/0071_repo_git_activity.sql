-- The node owns repository filesystems. It periodically materializes Git
-- activity here so control-plane metrics remain filesystem-independent.
ALTER TABLE repo_runtime_state
    ADD COLUMN git_activity_json JSONB,
    ADD COLUMN git_activity_collected_at TIMESTAMPTZ,
    ADD COLUMN next_git_activity_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ADD COLUMN git_activity_error TEXT;

CREATE INDEX repo_runtime_state_git_activity_due_idx
    ON repo_runtime_state(next_git_activity_at)
    WHERE exists = TRUE;
