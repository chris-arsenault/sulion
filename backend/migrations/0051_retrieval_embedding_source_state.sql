-- Durable source-state queue for retrieval semantic indexing. This stores
-- source keys and hashes only; transcript text remains in canonical tables.

CREATE TABLE IF NOT EXISTS retrieval_embedding_sources (
    id BIGSERIAL PRIMARY KEY,
    source_family TEXT NOT NULL CHECK (source_family IN ('event_block', 'operation_call', 'operation_result')),
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
    source_key TEXT NOT NULL UNIQUE,
    session_uuid UUID NOT NULL REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    byte_offset BIGINT,
    block_ord INT,
    turn_id BIGINT,
    operation_ord INT,
    repo_name TEXT,
    content_hash TEXT NOT NULL,
    last_seen_generation BIGINT,
    index_status TEXT NOT NULL DEFAULT 'pending'
        CHECK (index_status IN ('pending', 'indexed', 'failed', 'deleted')),
    index_error TEXT,
    last_seen_at TIMESTAMPTZ,
    dirty_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    indexed_at TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS retrieval_embedding_sources_pending_idx
    ON retrieval_embedding_sources(dirty_at, id)
    WHERE index_status = 'pending' AND deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS retrieval_embedding_sources_repo_status_idx
    ON retrieval_embedding_sources(repo_name, index_status)
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS retrieval_embedding_sources_family_scope_idx
    ON retrieval_embedding_sources(source_family, repo_name, session_uuid)
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS retrieval_embedding_sources_session_status_idx
    ON retrieval_embedding_sources(session_uuid, index_status)
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS retrieval_embedding_sources_generation_idx
    ON retrieval_embedding_sources(last_seen_generation)
    WHERE deleted_at IS NULL;

CREATE SEQUENCE IF NOT EXISTS retrieval_embedding_backfill_generation_seq;

CREATE TABLE IF NOT EXISTS retrieval_embedding_backfills (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    generation BIGINT NOT NULL,
    source_family TEXT NOT NULL CHECK (source_family IN ('event_block', 'operation_call', 'operation_result')),
    scope_repo TEXT,
    scope_session_uuid UUID,
    force BOOLEAN NOT NULL DEFAULT FALSE,
    status TEXT NOT NULL DEFAULT 'running'
        CHECK (status IN ('running', 'complete', 'failed', 'cancelled')),
    cursor_session_uuid UUID,
    cursor_byte_offset BIGINT,
    cursor_block_ord INT,
    cursor_turn_id BIGINT,
    cursor_operation_ord INT,
    rows_seen BIGINT NOT NULL DEFAULT 0,
    rows_marked_pending BIGINT NOT NULL DEFAULT 0,
    last_error TEXT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS retrieval_embedding_backfills_running_idx
    ON retrieval_embedding_backfills(updated_at, id)
    WHERE status = 'running';

CREATE INDEX IF NOT EXISTS retrieval_embedding_backfills_generation_idx
    ON retrieval_embedding_backfills(generation, status);
