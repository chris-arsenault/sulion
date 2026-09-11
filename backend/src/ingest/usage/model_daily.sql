WITH codex_raw AS (
    SELECT e.session_uuid, e.agent, e.byte_offset,
        (e.timestamp AT TIME ZONE 'UTC')::DATE AS day,
        COALESCE(
            (SELECT prior.payload #>> '{payload,model}'
               FROM events prior
              WHERE prior.session_uuid = e.session_uuid
                AND prior.byte_offset <= e.byte_offset
                AND prior.payload ->> 'type' = 'turn_context'
                AND prior.payload #>> '{payload,model}' IS NOT NULL
              ORDER BY prior.byte_offset DESC LIMIT 1),
            '(unknown model)') AS model,
        e.payload #> '{payload,info,total_token_usage}' AS usage
    FROM events e
    WHERE e.agent = 'codex'
      AND e.payload #>> '{payload,type}' = 'token_count'
      AND jsonb_typeof(e.payload #> '{payload,info,total_token_usage}') = 'object'
 ), codex_parsed AS (
    SELECT *,
        CASE WHEN jsonb_typeof(usage -> 'input_tokens') = 'number'
            THEN (usage ->> 'input_tokens')::NUMERIC::BIGINT ELSE 0 END AS reported_input,
        CASE WHEN jsonb_typeof(usage -> 'cached_input_tokens') = 'number'
            THEN (usage ->> 'cached_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_read,
        CASE WHEN jsonb_typeof(usage -> 'cache_write_input_tokens') = 'number'
            THEN (usage ->> 'cache_write_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_write,
        CASE WHEN jsonb_typeof(usage -> 'output_tokens') = 'number'
            THEN (usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output
    FROM codex_raw
 ), codex_normalized AS (
    SELECT *, GREATEST(reported_input - cache_read - cache_write, 0) AS input
    FROM codex_parsed
 ), codex_seq AS (
    SELECT *, LAG(input) OVER w AS prev_input,
        LAG(cache_read) OVER w AS prev_cache_read,
        LAG(cache_write) OVER w AS prev_cache_write,
        LAG(output) OVER w AS prev_output
    FROM codex_normalized
    WINDOW w AS (PARTITION BY session_uuid ORDER BY byte_offset)
 ), codex_daily AS (
    SELECT day, session_uuid, MIN(agent) AS agent, model,
        SUM(GREATEST(input - COALESCE(prev_input, 0), 0))::BIGINT AS input,
        SUM(GREATEST(cache_read - COALESCE(prev_cache_read, 0), 0))::BIGINT AS cache_read,
        SUM(GREATEST(cache_write - COALESCE(prev_cache_write, 0), 0))::BIGINT AS cache_write,
        0::BIGINT AS cache_write_1h,
        SUM(GREATEST(output - COALESCE(prev_output, 0), 0))::BIGINT AS output,
        NULL::TEXT AS last_message_id
    FROM codex_seq GROUP BY day, session_uuid, model
 ), claude_raw AS (
    SELECT e.session_uuid, e.agent, e.byte_offset,
        (e.timestamp AT TIME ZONE 'UTC')::DATE AS day,
        COALESCE(e.payload #>> '{message,id}', e.byte_offset::TEXT) AS response_key,
        e.payload #>> '{message,id}' AS message_id,
        COALESCE(e.payload #>> '{message,model}', '(unknown model)') AS model,
        COALESCE(e.payload #> '{message,usage}', e.payload -> 'usage') AS usage
    FROM events e
    WHERE e.agent = 'claude-code' AND e.payload ->> 'type' = 'assistant'
 ), claude_responses AS (
    SELECT DISTINCT ON (session_uuid, response_key) *
    FROM claude_raw WHERE jsonb_typeof(usage) = 'object'
    ORDER BY session_uuid, response_key, byte_offset DESC
 ), claude_parsed AS (
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
            THEN (usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output
    FROM claude_responses
 ), claude_daily AS (
    SELECT day, session_uuid, MIN(agent) AS agent, model,
        SUM(GREATEST(input, 0))::BIGINT AS input,
        SUM(GREATEST(cache_read, 0))::BIGINT AS cache_read,
        SUM(GREATEST(cache_total - cache_1h, cache_5m, 0))::BIGINT AS cache_write,
        SUM(GREATEST(cache_1h, 0))::BIGINT AS cache_write_1h,
        SUM(GREATEST(output, 0))::BIGINT AS output,
        (ARRAY_AGG(message_id ORDER BY byte_offset DESC))[1] AS last_message_id
    FROM claude_parsed GROUP BY day, session_uuid, model
 ), model_days AS (
    SELECT * FROM codex_daily UNION ALL SELECT * FROM claude_daily
 )
 INSERT INTO agent_model_usage_daily (
    day, session_uuid, agent, model, input_tokens, cached_input_tokens,
    cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens,
    last_usage_message_id, updated_at)
 SELECT day, session_uuid, agent, model, input, cache_read, cache_write,
    cache_write_1h, output, last_message_id, NOW() FROM model_days;
