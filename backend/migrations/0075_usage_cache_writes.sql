-- Preserve cache writes as their own usage categories. They are input tokens,
-- but providers price ordinary/5-minute writes and Claude 1-hour writes
-- differently from both standard input and cache reads.

ALTER TABLE agent_session_usage
    ADD COLUMN cache_write_input_tokens BIGINT NOT NULL DEFAULT 0
        CHECK (cache_write_input_tokens >= 0),
    ADD COLUMN cache_write_1h_input_tokens BIGINT NOT NULL DEFAULT 0
        CHECK (cache_write_1h_input_tokens >= 0);

ALTER TABLE agent_usage_daily
    ADD COLUMN cache_write_input_tokens BIGINT NOT NULL DEFAULT 0
        CHECK (cache_write_input_tokens >= 0),
    ADD COLUMN cache_write_1h_input_tokens BIGINT NOT NULL DEFAULT 0
        CHECK (cache_write_1h_input_tokens >= 0);

-- Direct daily deltas by the model reported at each response/turn. A session
-- can switch models, so joining session totals to the latest metadata would
-- misprice earlier turns.
CREATE TABLE agent_model_usage_daily (
    day DATE NOT NULL,
    session_uuid UUID NOT NULL REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    agent TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    cached_input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (cached_input_tokens >= 0),
    cache_write_input_tokens BIGINT NOT NULL DEFAULT 0
        CHECK (cache_write_input_tokens >= 0),
    cache_write_1h_input_tokens BIGINT NOT NULL DEFAULT 0
        CHECK (cache_write_1h_input_tokens >= 0),
    output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    last_usage_message_id TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (day, session_uuid, model)
);

CREATE INDEX agent_model_usage_daily_day_model_idx
    ON agent_model_usage_daily(day, model);
