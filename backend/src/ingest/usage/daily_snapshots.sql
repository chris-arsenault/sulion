WITH codex_raw AS (
    SELECT DISTINCT ON (e.session_uuid, (e.timestamp AT TIME ZONE 'UTC')::DATE)
        (e.timestamp AT TIME ZONE 'UTC')::DATE AS day,
        e.session_uuid, e.agent,
        e.payload #> '{payload,info,total_token_usage}' AS usage
    FROM events e
    WHERE e.agent = 'codex'
      AND e.subtype IS DISTINCT FROM 'inherited_history'
      AND e.payload #>> '{payload,type}' = 'token_count'
      AND jsonb_typeof(e.payload #> '{payload,info,total_token_usage}') = 'object'
    ORDER BY e.session_uuid, (e.timestamp AT TIME ZONE 'UTC')::DATE, e.byte_offset DESC
 ), codex AS (
    SELECT day, session_uuid, agent,
        CASE WHEN jsonb_typeof(usage -> 'input_tokens') = 'number'
            THEN (usage ->> 'input_tokens')::NUMERIC::BIGINT ELSE 0 END AS reported_input,
        CASE WHEN jsonb_typeof(usage -> 'cached_input_tokens') = 'number'
            THEN (usage ->> 'cached_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_read,
        CASE WHEN jsonb_typeof(usage -> 'cache_write_input_tokens') = 'number'
            THEN (usage ->> 'cache_write_input_tokens')::NUMERIC::BIGINT ELSE 0 END AS cache_write,
        CASE WHEN jsonb_typeof(usage -> 'output_tokens') = 'number'
            THEN (usage ->> 'output_tokens')::NUMERIC::BIGINT ELSE 0 END AS output,
        CASE WHEN jsonb_typeof(usage -> 'reasoning_output_tokens') = 'number'
            THEN (usage ->> 'reasoning_output_tokens')::NUMERIC::BIGINT ELSE 0 END AS reasoning,
        CASE WHEN jsonb_typeof(usage -> 'total_tokens') = 'number'
            THEN (usage ->> 'total_tokens')::NUMERIC::BIGINT ELSE NULL END AS reported_total
    FROM codex_raw
 ), claude_raw AS (
    SELECT e.session_uuid, e.agent, e.byte_offset,
        (e.timestamp AT TIME ZONE 'UTC')::DATE AS day,
        COALESCE(e.payload #>> '{message,id}', e.byte_offset::TEXT) AS response_key,
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
 ), claude_by_day AS (
    SELECT day, session_uuid, MIN(agent) AS agent,
        SUM(GREATEST(input, 0))::BIGINT AS input,
        SUM(GREATEST(cache_read, 0))::BIGINT AS cache_read,
        SUM(GREATEST(cache_total - cache_1h, cache_5m, 0))::BIGINT AS cache_write,
        SUM(GREATEST(cache_1h, 0))::BIGINT AS cache_write_1h,
        SUM(GREATEST(output, 0))::BIGINT AS output
    FROM claude_parsed GROUP BY day, session_uuid
 ), claude AS (
    SELECT day, session_uuid, agent,
        SUM(input) OVER w AS input, SUM(cache_read) OVER w AS cache_read,
        SUM(cache_write) OVER w AS cache_write,
        SUM(cache_write_1h) OVER w AS cache_write_1h,
        SUM(output) OVER w AS output
    FROM claude_by_day
    WINDOW w AS (PARTITION BY session_uuid ORDER BY day ROWS UNBOUNDED PRECEDING)
 ), snapshots AS (
    SELECT day, session_uuid, agent,
        GREATEST(reported_input - cache_read - cache_write, 0)::BIGINT AS input,
        GREATEST(cache_read, 0)::BIGINT AS cache_read,
        GREATEST(cache_write, 0)::BIGINT AS cache_write, 0::BIGINT AS cache_write_1h,
        GREATEST(output, 0)::BIGINT AS output, GREATEST(reasoning, 0)::BIGINT AS reasoning,
        GREATEST(COALESCE(reported_total, reported_input + output), 0)::BIGINT AS total
    FROM codex
    UNION ALL
    SELECT day, session_uuid, agent, input, cache_read, cache_write, cache_write_1h,
        output, 0, input + cache_read + cache_write + cache_write_1h + output
    FROM claude
 )
 INSERT INTO agent_usage_daily (
    day, session_uuid, agent, input_tokens, cached_input_tokens,
    cache_write_input_tokens, cache_write_1h_input_tokens, output_tokens,
    reasoning_output_tokens, total_tokens, updated_at)
 SELECT day, session_uuid, agent, input, cache_read, cache_write, cache_write_1h,
    output, reasoning, total, NOW() FROM snapshots;
