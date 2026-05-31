-- Agent-facing retrieval uses existing transcript/timeline tables as the
-- source of truth. This schema stores embeddings and source keys only; it
-- does not copy transcript text into a second corpus.

CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE TABLE IF NOT EXISTS retrieval_embeddings (
    id BIGSERIAL PRIMARY KEY,
    source_kind TEXT NOT NULL CHECK (
        source_kind IN (
            'assistant_text',
            'user_prompt',
            'summary',
            'tool_call',
            'tool_result',
            'tool_error',
            'turn_digest'
        )
    ),
    source_key TEXT NOT NULL,
    session_uuid UUID NOT NULL REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    byte_offset BIGINT,
    block_ord INT,
    turn_id BIGINT,
    operation_ord INT,
    repo_name TEXT,
    content_hash TEXT NOT NULL,
    embedding_model TEXT NOT NULL,
    embedding_dimensions INT NOT NULL,
    embedding REAL[] NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (embedding_model, source_key)
);
