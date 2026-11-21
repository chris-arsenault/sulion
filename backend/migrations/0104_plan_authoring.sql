ALTER TABLE plans
    ADD COLUMN authoring_revision BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN authoring JSONB;

ALTER TABLE plan_events
    ADD COLUMN authoring_revision BIGINT,
    ADD COLUMN authoring_change JSONB;

ALTER TABLE plan_phases ADD COLUMN retired_at TIMESTAMPTZ;
