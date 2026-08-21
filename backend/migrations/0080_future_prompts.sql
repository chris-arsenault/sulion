-- Session-scoped deferred prompts move from node-local markdown files
-- into the database, same reasoning as library_entries: the process
-- answering the API must see them regardless of topology.
CREATE TABLE future_prompts (
    session_uuid UUID NOT NULL,
    id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    text TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (session_uuid, id)
);

CREATE INDEX future_prompts_pending_idx ON future_prompts (session_uuid) WHERE state = 'pending';
