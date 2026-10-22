ALTER TABLE plans
    ADD COLUMN boundaries JSONB NOT NULL DEFAULT '[]'::jsonb
    CHECK (jsonb_typeof(boundaries) = 'array');
