-- Incremental timeline projection. Each event appends its own item and, for a
-- tool call, its operation; a result updates only that operation. The reducer
-- resumes from the cursor and open-turn pointers below. Existing rows are
-- rebuilt from canonical events by the projection version bump that ships
-- with this schema.

ALTER TABLE timeline_session_state
    ADD COLUMN projected_through BIGINT NOT NULL DEFAULT -1,
    ADD COLUMN projection_version INT NOT NULL DEFAULT 0,
    ADD COLUMN next_turn_ord INT NOT NULL DEFAULT 0,
    ADD COLUMN current_main_turn_id BIGINT,
    ADD COLUMN current_sidechain_turn_id BIGINT,
    ADD COLUMN codex_input_total BIGINT,
    ADD COLUMN codex_output_total BIGINT,
    -- Set when a rebuild clears the session; its operation retrieval sources
    -- are reconciled once the rebuild catches up.
    ADD COLUMN reconcile_sources BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE timeline_turns
    DROP COLUMN chunks_json,
    ADD COLUMN prompt_event_uuid TEXT,
    ADD COLUMN usage_baseline_input BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN usage_baseline_output BIGINT NOT NULL DEFAULT 0;

CREATE INDEX timeline_turns_prompt_uuid_idx
    ON timeline_turns(session_uuid, prompt_event_uuid)
    WHERE prompt_event_uuid IS NOT NULL;

-- One visible event of a turn. Written once; readers group consecutive
-- assistant items.
CREATE TABLE timeline_items (
    session_uuid UUID NOT NULL,
    turn_id BIGINT NOT NULL,
    byte_offset BIGINT NOT NULL,
    body JSONB NOT NULL,
    PRIMARY KEY (session_uuid, turn_id, byte_offset),
    FOREIGN KEY (session_uuid, turn_id)
        REFERENCES timeline_turns(session_uuid, turn_id) ON DELETE CASCADE
);

-- `changed_at` is the byte offset of the event that last changed the row, so
-- an open view asks for operations changed after the offset it has read.
ALTER TABLE timeline_operations
    DROP COLUMN subagent_json,
    ADD COLUMN call_offset BIGINT,
    ADD COLUMN call_at TIMESTAMPTZ,
    ADD COLUMN changed_at BIGINT NOT NULL DEFAULT 0,
    -- The call's own error: its block flag or failed runtime evidence.
    ADD COLUMN call_error BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN running_cell TEXT,
    ADD COLUMN finished_at TIMESTAMPTZ;

CREATE INDEX timeline_operations_session_pair_idx
    ON timeline_operations(session_uuid, pair_id, call_offset);

CREATE INDEX timeline_operations_changed_idx
    ON timeline_operations(session_uuid, turn_id, changed_at);

CREATE INDEX timeline_operations_exec_finish_idx
    ON timeline_operations(session_uuid, finished_at)
    WHERE raw_name = 'exec' OR raw_name LIKE '%.exec';

CREATE INDEX timeline_operations_running_cell_idx
    ON timeline_operations(session_uuid, running_cell)
    WHERE running_cell IS NOT NULL;

-- A Claude response's latest usage and the turn it is charged to. Streamed
-- blocks revise a response's usage; the newer value replaces the older.
CREATE TABLE timeline_message_usage (
    session_uuid UUID NOT NULL REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    message_id TEXT NOT NULL,
    turn_id BIGINT NOT NULL,
    input_tokens BIGINT NOT NULL,
    output_tokens BIGINT NOT NULL,
    PRIMARY KEY (session_uuid, message_id)
);

-- A spawning call and the transcript it produced: a whole child session
-- (child_turn_id = -1) or a sidechain turn of the same session. Either side
-- may be ingested first, so neither end is a foreign key.
CREATE TABLE timeline_child_links (
    session_uuid UUID NOT NULL,
    pair_id TEXT NOT NULL,
    child_session_uuid UUID NOT NULL,
    child_turn_id BIGINT NOT NULL DEFAULT -1,
    PRIMARY KEY (session_uuid, pair_id, child_session_uuid, child_turn_id)
);

CREATE INDEX timeline_child_links_child_idx
    ON timeline_child_links(child_session_uuid);

-- Written on every projection and read by nothing.
DROP TABLE timeline_activity_signals;
