-- Finish the canonical event model by promoting the remaining
-- frontend-relevant metadata out of events.payload. Raw payload stays
-- stored for re-derivation/backfill only; normal API consumers should
-- read these structured columns instead.

ALTER TABLE events
    ADD COLUMN event_uuid TEXT,
    ADD COLUMN parent_event_uuid TEXT,
    ADD COLUMN related_tool_use_id TEXT,
    ADD COLUMN is_sidechain BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN is_meta BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN subtype TEXT,
    ADD COLUMN search_text TEXT NOT NULL DEFAULT '';

CREATE INDEX events_event_uuid_idx
    ON events(event_uuid)
    WHERE event_uuid IS NOT NULL;

CREATE INDEX events_parent_event_uuid_idx
    ON events(parent_event_uuid)
    WHERE parent_event_uuid IS NOT NULL;

CREATE INDEX events_related_tool_use_id_idx
    ON events(related_tool_use_id)
    WHERE related_tool_use_id IS NOT NULL;
