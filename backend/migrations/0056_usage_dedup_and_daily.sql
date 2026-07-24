-- Token accounting fixes and daily rollups.
--
-- 1. `last_usage_message_id` — Claude Code writes one JSONL line per content
--    block of an API response; every line repeats the same `message.usage`.
--    The ingester dedupes by remembering the last counted message id.
-- 2. `agent_usage_daily` — end-of-day cumulative snapshots per session.
--    "Tokens on day D" is snapshot(D) minus the previous snapshot, so daily
--    series need no separate delta bookkeeping in the hot ingest path.
-- 3. `usage_backfills` — marker table for the nonblocking startup task that
--    recomputes historical Claude rows (dedup + cache-read split). DDL only
--    here; the heavy recompute runs in the background after boot.

ALTER TABLE agent_session_usage
    ADD COLUMN last_usage_message_id TEXT;

CREATE TABLE agent_usage_daily (
    day DATE NOT NULL,
    session_uuid UUID NOT NULL REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    agent TEXT NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    cached_input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (cached_input_tokens >= 0),
    output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    reasoning_output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (reasoning_output_tokens >= 0),
    total_tokens BIGINT NOT NULL DEFAULT 0 CHECK (total_tokens >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (day, session_uuid)
);

CREATE INDEX agent_usage_daily_session_idx
    ON agent_usage_daily(session_uuid, day);

CREATE TABLE usage_backfills (
    id TEXT PRIMARY KEY,
    completed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
