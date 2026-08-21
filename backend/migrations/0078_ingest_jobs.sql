-- Progress records for long-running background work (startup backfills,
-- transcript catch-up). One row per run; the UI polls these instead of
-- leaving catch-up periods invisible.
CREATE TABLE ingest_jobs (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    label TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'running',
    progress_current BIGINT NOT NULL DEFAULT 0,
    -- NULL means indeterminate (no known total).
    progress_total BIGINT,
    unit TEXT NOT NULL DEFAULT 'items',
    detail TEXT,
    error TEXT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);

CREATE INDEX ingest_jobs_running_idx ON ingest_jobs (name) WHERE status = 'running';
CREATE INDEX ingest_jobs_finished_idx ON ingest_jobs (finished_at DESC) WHERE finished_at IS NOT NULL;
