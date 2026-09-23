ALTER TABLE plans
    ADD COLUMN outcome TEXT NOT NULL DEFAULT '',
    ADD COLUMN principles TEXT[] NOT NULL DEFAULT '{}',
    ADD COLUMN assumptions TEXT[] NOT NULL DEFAULT '{}';

ALTER TABLE plan_events
    ADD COLUMN guidance_before JSONB,
    ADD COLUMN guidance_after JSONB;
