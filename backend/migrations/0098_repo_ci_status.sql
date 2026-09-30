-- The node records each repository's origin remote with its Git status and
-- polls the public GitHub Actions API for the latest run on the checked-out
-- branch. The web URL and owner/name are derived from origin_url on read.
ALTER TABLE repo_runtime_state
    ADD COLUMN origin_url TEXT,
    ADD COLUMN ci_state TEXT CHECK (ci_state IN ('failed', 'in_progress', 'succeeded')),
    ADD COLUMN ci_branch TEXT,
    ADD COLUMN ci_run_url TEXT,
    ADD COLUMN ci_run_updated_at TIMESTAMPTZ,
    ADD COLUMN ci_checked_at TIMESTAMPTZ,
    ADD COLUMN ci_error TEXT,
    ADD COLUMN next_ci_at TIMESTAMPTZ NOT NULL DEFAULT NOW();

-- Spread the first checks over an hour rather than spending the anonymous
-- hourly budget in one burst on upgrade.
UPDATE repo_runtime_state SET next_ci_at = NOW() + random() * INTERVAL '1 hour';
