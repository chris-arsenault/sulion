WITH raw AS (
    SELECT DISTINCT ON (e.session_uuid)
        e.session_uuid, e.agent, e.byte_offset, e.timestamp,
        e.payload #> '{payload,info,total_token_usage}' AS total_usage,
        e.payload #> '{payload,info,last_token_usage}' AS last_usage,
        e.payload #> '{payload,info,model_context_window}' AS context_window
    FROM events e
    WHERE e.agent = 'codex'
      AND e.payload #>> '{payload,type}' = 'token_count'
      AND jsonb_typeof(e.payload #> '{payload,info,total_token_usage}') = 'object'
    ORDER BY e.session_uuid, e.byte_offset DESC
 ), parsed AS (
    SELECT *,
        CASE WHEN jsonb_typeof(total_usage -> 'input_tokens') = 'number'
            THEN (total_usage ->> 'input_tokens')::NUMERIC::BIGINT ELSE 0 END AS reported_input,
        CASE WHEN jsonb_typeof(total_usage -> 'cached_input_tokens') = 'number'
            THEN (total_usage ->> 'cached_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_read,
        CASE WHEN jsonb_typeof(total_usage -> 'cache_write_input_tokens') = 'number'
            THEN (total_usage ->> 'cache_write_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_write,
        CASE WHEN jsonb_typeof(total_usage -> 'output_tokens') = 'number'
            THEN (total_usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output,
        CASE WHEN jsonb_typeof(total_usage -> 'reasoning_output_tokens') = 'number'
            THEN (total_usage ->> 'reasoning_output_tokens')::NUMERIC::BIGINT ELSE 0 END AS reasoning,
        CASE WHEN jsonb_typeof(total_usage -> 'total_tokens') = 'number'
            THEN (total_usage ->> 'total_tokens')::NUMERIC::BIGINT ELSE NULL END AS reported_total,
        CASE WHEN jsonb_typeof(last_usage -> 'total_tokens') = 'number'
            THEN (last_usage ->> 'total_tokens')::NUMERIC::BIGINT ELSE NULL END AS last_total,
        CASE WHEN jsonb_typeof(last_usage -> 'reasoning_output_tokens') = 'number'
            THEN (last_usage ->> 'reasoning_output_tokens')::NUMERIC::BIGINT ELSE 0 END AS last_reasoning,
        CASE WHEN jsonb_typeof(context_window) = 'number'
            THEN (context_window #>> '{}')::NUMERIC::BIGINT ELSE NULL END AS reported_window
    FROM raw
 )
 INSERT INTO agent_session_usage (
    session_uuid, agent, input_tokens, cached_input_tokens,
    cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens,
    reasoning_output_tokens, total_tokens, context_tokens, model_context_window,
    last_byte_offset, observed_at, last_usage_message_id, updated_at)
 SELECT session_uuid, agent,
    GREATEST(reported_input - cache_read - cache_write, 0),
    GREATEST(cache_read, 0), GREATEST(cache_write, 0), 0,
    GREATEST(output, 0), GREATEST(reasoning, 0),
    GREATEST(COALESCE(reported_total, reported_input + output), 0),
    CASE WHEN last_total IS NULL THEN NULL
         ELSE GREATEST(last_total - last_reasoning, 0) END,
    CASE WHEN reported_window > 0 THEN reported_window ELSE NULL END,
    byte_offset, timestamp, NULL, NOW()
 FROM parsed;
