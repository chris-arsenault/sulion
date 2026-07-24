-- Normalized, session-scoped token usage reported by agent transcripts.
--
-- Token spend and context pressure are deliberately separate. `total_tokens`
-- is cumulative work for the agent session, while `context_tokens` is the
-- latest model call's estimated context footprint.

CREATE TABLE agent_session_usage (
    session_uuid UUID PRIMARY KEY REFERENCES claude_sessions(session_uuid) ON DELETE CASCADE,
    agent TEXT NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    cached_input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (cached_input_tokens >= 0),
    output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    reasoning_output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (reasoning_output_tokens >= 0),
    total_tokens BIGINT NOT NULL DEFAULT 0 CHECK (total_tokens >= 0),
    context_tokens BIGINT CHECK (context_tokens >= 0),
    model_context_window BIGINT CHECK (model_context_window > 0),
    last_byte_offset BIGINT NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX agent_session_usage_updated_idx
    ON agent_session_usage(updated_at DESC);

-- Existing Codex sessions publish cumulative snapshots, so the newest usable
-- snapshot is the complete session total.
WITH codex_raw AS (
    SELECT DISTINCT ON (e.session_uuid)
        e.session_uuid,
        e.agent,
        e.byte_offset,
        e.timestamp,
        e.payload #> '{payload,info,total_token_usage}' AS total_usage,
        e.payload #> '{payload,info,last_token_usage}' AS last_usage,
        e.payload #> '{payload,info,model_context_window}' AS context_window
    FROM events e
    WHERE e.agent = 'codex'
      AND e.payload #>> '{payload,type}' = 'token_count'
      AND jsonb_typeof(e.payload #> '{payload,info,total_token_usage}') = 'object'
    ORDER BY e.session_uuid, e.byte_offset DESC
),
codex_parsed AS (
    SELECT
        *,
        CASE WHEN jsonb_typeof(total_usage -> 'input_tokens') = 'number'
            THEN (total_usage ->> 'input_tokens')::NUMERIC::BIGINT ELSE 0 END AS input_tokens,
        CASE WHEN jsonb_typeof(total_usage -> 'cached_input_tokens') = 'number'
            THEN (total_usage ->> 'cached_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cached_input_tokens,
        CASE WHEN jsonb_typeof(total_usage -> 'output_tokens') = 'number'
            THEN (total_usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output_tokens,
        CASE WHEN jsonb_typeof(total_usage -> 'reasoning_output_tokens') = 'number'
            THEN (total_usage ->> 'reasoning_output_tokens')::NUMERIC::BIGINT ELSE 0 END AS reasoning_tokens,
        CASE WHEN jsonb_typeof(total_usage -> 'total_tokens') = 'number'
            THEN (total_usage ->> 'total_tokens')::NUMERIC::BIGINT ELSE NULL END AS reported_total,
        CASE WHEN jsonb_typeof(last_usage -> 'total_tokens') = 'number'
            THEN (last_usage ->> 'total_tokens')::NUMERIC::BIGINT ELSE NULL END AS last_total,
        CASE WHEN jsonb_typeof(last_usage -> 'reasoning_output_tokens') = 'number'
            THEN (last_usage ->> 'reasoning_output_tokens')::NUMERIC::BIGINT ELSE 0 END AS last_reasoning,
        CASE WHEN jsonb_typeof(context_window) = 'number'
            THEN (context_window #>> '{}')::NUMERIC::BIGINT ELSE NULL END AS reported_window
    FROM codex_raw
)
INSERT INTO agent_session_usage (
    session_uuid, agent, input_tokens, cached_input_tokens, output_tokens,
    reasoning_output_tokens, total_tokens, context_tokens, model_context_window,
    last_byte_offset, observed_at
)
SELECT
    session_uuid,
    agent,
    GREATEST(input_tokens, 0),
    GREATEST(cached_input_tokens, 0),
    GREATEST(output_tokens, 0),
    GREATEST(reasoning_tokens, 0),
    GREATEST(COALESCE(reported_total, input_tokens + output_tokens), 0),
    CASE WHEN last_total IS NULL THEN NULL
        ELSE GREATEST(last_total - last_reasoning, 0) END,
    CASE WHEN reported_window > 0 THEN reported_window ELSE NULL END,
    byte_offset,
    timestamp
FROM codex_parsed
ON CONFLICT (session_uuid) DO NOTHING;

-- Claude assistant usage is per API response. Sum every response for session
-- spend, while retaining only the newest response as context pressure.
WITH claude_raw AS (
    SELECT
        e.session_uuid,
        e.agent,
        e.byte_offset,
        e.timestamp,
        COALESCE(e.payload #> '{message,usage}', e.payload -> 'usage') AS usage
    FROM events e
    WHERE e.agent = 'claude-code'
      AND e.payload ->> 'type' = 'assistant'
),
claude_parsed AS (
    SELECT
        *,
        CASE WHEN jsonb_typeof(usage -> 'input_tokens') = 'number'
            THEN (usage ->> 'input_tokens')::NUMERIC::BIGINT ELSE 0 END AS input_tokens,
        CASE WHEN jsonb_typeof(usage -> 'cache_creation_input_tokens') = 'number'
            THEN (usage ->> 'cache_creation_input_tokens')::NUMERIC::BIGINT ELSE 0 END
        + CASE WHEN jsonb_typeof(usage -> 'cache_read_input_tokens') = 'number'
            THEN (usage ->> 'cache_read_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cached_tokens,
        CASE WHEN jsonb_typeof(usage -> 'output_tokens') = 'number'
            THEN (usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output_tokens
    FROM claude_raw
    WHERE jsonb_typeof(usage) = 'object'
),
claude_totals AS (
    SELECT
        session_uuid,
        MIN(agent) AS agent,
        SUM(GREATEST(input_tokens, 0))::BIGINT AS input_tokens,
        SUM(GREATEST(cached_tokens, 0))::BIGINT AS cached_tokens,
        SUM(GREATEST(output_tokens, 0))::BIGINT AS output_tokens,
        (
            ARRAY_AGG(
                GREATEST(input_tokens + cached_tokens + output_tokens, 0)
                ORDER BY byte_offset DESC
            )
        )[1]::BIGINT AS context_tokens,
        MAX(byte_offset) AS last_byte_offset,
        (ARRAY_AGG(timestamp ORDER BY byte_offset DESC))[1] AS observed_at
    FROM claude_parsed
    GROUP BY session_uuid
)
INSERT INTO agent_session_usage (
    session_uuid, agent, input_tokens, cached_input_tokens, output_tokens,
    reasoning_output_tokens, total_tokens, context_tokens, model_context_window,
    last_byte_offset, observed_at
)
SELECT
    session_uuid,
    agent,
    input_tokens,
    cached_tokens,
    output_tokens,
    0,
    input_tokens + cached_tokens + output_tokens,
    context_tokens,
    NULL,
    last_byte_offset,
    observed_at
FROM claude_totals
ON CONFLICT (session_uuid) DO NOTHING;
