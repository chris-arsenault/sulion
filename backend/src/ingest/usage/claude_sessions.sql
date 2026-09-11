WITH raw AS (
    SELECT e.session_uuid, e.agent, e.byte_offset, e.timestamp,
        COALESCE(e.payload #>> '{message,id}', e.byte_offset::TEXT) AS response_key,
        e.payload #>> '{message,id}' AS message_id,
        COALESCE(e.payload #> '{message,usage}', e.payload -> 'usage') AS usage
    FROM events e
    WHERE e.agent = 'claude-code' AND e.payload ->> 'type' = 'assistant'
 ), responses AS (
    SELECT DISTINCT ON (session_uuid, response_key) *
    FROM raw WHERE jsonb_typeof(usage) = 'object'
    ORDER BY session_uuid, response_key, byte_offset DESC
 ), parsed AS (
    SELECT *,
        CASE WHEN jsonb_typeof(usage -> 'input_tokens') = 'number'
            THEN (usage ->> 'input_tokens')::NUMERIC::BIGINT ELSE 0 END AS input,
        CASE WHEN jsonb_typeof(usage -> 'cache_read_input_tokens') = 'number'
            THEN (usage ->> 'cache_read_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_read,
        CASE WHEN jsonb_typeof(usage -> 'cache_creation_input_tokens') = 'number'
            THEN (usage ->> 'cache_creation_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_total,
        CASE WHEN jsonb_typeof(usage #> '{cache_creation,ephemeral_5m_input_tokens}') = 'number'
            THEN (usage #>> '{cache_creation,ephemeral_5m_input_tokens}')::NUMERIC::BIGINT ELSE 0 END AS cache_5m,
        CASE WHEN jsonb_typeof(usage #> '{cache_creation,ephemeral_1h_input_tokens}') = 'number'
            THEN (usage #>> '{cache_creation,ephemeral_1h_input_tokens}')::NUMERIC::BIGINT ELSE 0 END AS cache_1h,
        CASE WHEN jsonb_typeof(usage -> 'output_tokens') = 'number'
            THEN (usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output,
        CASE WHEN jsonb_typeof(usage -> 'model_context_window') = 'number'
            THEN (usage ->> 'model_context_window')::NUMERIC::BIGINT ELSE NULL END AS reported_window
    FROM responses
 ), normalized AS (
    SELECT *, GREATEST(cache_total - cache_1h, cache_5m, 0) AS cache_write
    FROM parsed
 ), totals AS (
    SELECT session_uuid, MIN(agent) AS agent,
        SUM(GREATEST(input, 0))::BIGINT AS input,
        SUM(GREATEST(cache_read, 0))::BIGINT AS cache_read,
        SUM(GREATEST(cache_write, 0))::BIGINT AS cache_write,
        SUM(GREATEST(cache_1h, 0))::BIGINT AS cache_write_1h,
        SUM(GREATEST(output, 0))::BIGINT AS output,
        MAX(byte_offset) AS last_byte_offset,
        (ARRAY_AGG(timestamp ORDER BY byte_offset DESC))[1] AS observed_at,
        (ARRAY_AGG(message_id ORDER BY byte_offset DESC))[1] AS last_message_id,
        (ARRAY_AGG(
            GREATEST(input, 0) + GREATEST(cache_read, 0)
            + GREATEST(cache_write, 0) + GREATEST(cache_1h, 0)
            + GREATEST(output, 0) ORDER BY byte_offset DESC))[1]::BIGINT AS context_tokens,
        (ARRAY_AGG(reported_window ORDER BY byte_offset DESC)
            FILTER (WHERE reported_window > 0))[1] AS model_context_window
    FROM normalized GROUP BY session_uuid
 )
 INSERT INTO agent_session_usage (
    session_uuid, agent, input_tokens, cached_input_tokens,
    cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens,
    reasoning_output_tokens, total_tokens, context_tokens, model_context_window,
    last_byte_offset, observed_at, last_usage_message_id, updated_at)
 SELECT session_uuid, agent, input, cache_read, cache_write, cache_write_1h,
    output, 0, input + cache_read + cache_write + cache_write_1h + output,
    context_tokens, model_context_window, last_byte_offset, observed_at,
    last_message_id, NOW()
 FROM totals;
