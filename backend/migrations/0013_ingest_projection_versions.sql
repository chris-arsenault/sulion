-- Version gates for derived transcript data. These are not schema
-- migrations: they record which parser/projection repair passes have
-- already been applied to the existing Postgres transcript rows.

CREATE TABLE IF NOT EXISTS ingest_projection_versions (
    name TEXT PRIMARY KEY,
    version INT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
