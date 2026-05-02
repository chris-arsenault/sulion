-- Canonical event model. Raw JSONL payload stays untouched in
-- events.payload; we add a parallel structured representation that
-- every consumer (REST, frontend, search) can rely on without
-- reaching into an agent-specific shape.
--
-- Ingester parses each event into a list of canonical blocks on
-- insert. Historical repair is gated by ingest_projection_versions,
-- not run as unconditional startup work.

ALTER TABLE events
    ADD COLUMN agent TEXT NOT NULL DEFAULT 'claude-code',
    ADD COLUMN speaker TEXT,
    ADD COLUMN content_kind TEXT;

CREATE INDEX events_agent_idx ON events(agent);

CREATE TABLE event_blocks (
    id BIGSERIAL PRIMARY KEY,
    session_uuid UUID NOT NULL,
    byte_offset BIGINT NOT NULL,
    -- Index within the event's content list.
    ord INT NOT NULL,
    -- Canonical block kind: text | thinking | tool_use | tool_result | unknown.
    kind TEXT NOT NULL,
    -- Payloads (nullable depending on kind):
    text TEXT,
    tool_id TEXT,
    tool_name TEXT,
    tool_name_canonical TEXT,
    tool_input JSONB,
    is_error BOOLEAN,
    -- Raw block JSON for kind='unknown' so the frontend can render a
    -- "we don't know this yet" placeholder without losing information.
    raw JSONB,
    FOREIGN KEY (session_uuid, byte_offset)
        REFERENCES events(session_uuid, byte_offset) ON DELETE CASCADE,
    UNIQUE (session_uuid, byte_offset, ord)
);

CREATE INDEX event_blocks_event_idx ON event_blocks(session_uuid, byte_offset);
CREATE INDEX event_blocks_tool_canonical_idx
    ON event_blocks(tool_name_canonical)
    WHERE tool_name_canonical IS NOT NULL;
