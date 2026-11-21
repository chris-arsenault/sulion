WITH query AS (
    SELECT plainto_tsquery('simple', $1) AS terms
), catalog AS MATERIALIZED (
    SELECT * FROM plans p WHERE $2::TEXT IS NULL OR p.repo_name = $2
), links AS MATERIALIZED (
    SELECT p.id AS plan_id, s.phase_id, s.session_uuid, s.turn_id
    FROM catalog p
    CROSS JOIN LATERAL plan_turn_provenance(p.id) s
    WHERE $3::UUID IS NULL OR s.session_uuid = $3
    GROUP BY p.id, s.phase_id, s.session_uuid, s.turn_id
), documents AS (
    SELECT p.id AS plan_id, NULL::UUID AS phase_id,
           setweight(to_tsvector('simple', p.title), 'A') ||
           to_tsvector('simple', concat_ws(' ', p.summary, p.outcome,
               p.principles, p.assumptions, p.authoring->>'technical_details')) ||
           jsonb_to_tsvector('simple', COALESCE(p.authoring->'entries', '[]') || p.boundaries, '["string"]') AS body
    FROM catalog p
    WHERE ($3 IS NULL OR EXISTS (SELECT 1 FROM links l WHERE l.plan_id = p.id)
           OR p.created_by_agent_session_uuid = $3)
      AND ($4::TIMESTAMPTZ IS NULL OR p.updated_at >= $4)
      AND ($5::TIMESTAMPTZ IS NULL OR p.updated_at <= $5)
      AND $10::BOOLEAN AND $11::TEXT IS NULL AND $12::TEXT IS NULL
    UNION ALL
    SELECT p.id, ph.id,
           setweight(to_tsvector('simple', ph.title), 'A') ||
           to_tsvector('simple', concat_ws(' ', ph.description, ph.status_note)) ||
           jsonb_to_tsvector('simple', COALESCE(p.authoring->'phases'->ph.id::TEXT, '{}'), '["string"]')
    FROM catalog p JOIN plan_phases ph ON ph.plan_id = p.id
    WHERE ph.retired_at IS NULL
      AND ($3 IS NULL OR EXISTS (SELECT 1 FROM links l WHERE l.phase_id = ph.id))
      AND ($4 IS NULL OR ph.updated_at >= $4)
      AND ($5 IS NULL OR ph.updated_at <= $5)
      AND $10 AND $11 IS NULL AND $12 IS NULL
), prompts AS (
    SELECT l.*, CASE WHEN octet_length(t.user_prompt_text) <= 1000000
                     THEN to_tsvector('simple', t.user_prompt_text) END AS body
    FROM links l JOIN timeline_turns t USING (session_uuid, turn_id)
    JOIN claude_sessions s ON s.session_uuid = t.session_uuid
    LEFT JOIN agent_session_metadata asm ON asm.session_uuid = t.session_uuid
    WHERE $10 AND ($4 IS NULL OR t.start_timestamp >= $4)
      AND ($5 IS NULL OR t.start_timestamp <= $5)
      AND ($11 IS NULL OR s.agent = $11)
      AND ($12 IS NULL OR asm.model = $12)
), matches AS (
    SELECT d.plan_id, d.phase_id, NULL::UUID AS agent_session_uuid, NULL::BIGINT AS turn_id,
           ts_rank_cd(d.body, q.terms, 32) AS score, 1 AS priority
    FROM documents d CROSS JOIN query q
    WHERE d.body @@ q.terms
    UNION ALL
    SELECT p.plan_id, p.phase_id, p.session_uuid, p.turn_id,
           ts_rank_cd(p.body, q.terms, 32), 0
    FROM prompts p CROSS JOIN query q
    WHERE p.body @@ q.terms
    UNION ALL
    SELECT l.plan_id, l.phase_id, l.session_uuid, l.turn_id, hit.score, 0
    FROM unnest($6::UUID[], $7::BIGINT[], $8::REAL[]) hit(session_uuid, turn_id, score)
    JOIN links l USING (session_uuid, turn_id)
), best AS (
    SELECT DISTINCT ON (plan_id) * FROM matches
    ORDER BY plan_id, priority DESC, score DESC, phase_id NULLS LAST,
             agent_session_uuid, turn_id
)
SELECT p.id AS plan_id, p.repo_name AS repo, p.title, p.summary, p.status, p.root_plan_id,
       b.phase_id, ph.title AS phase_title, b.agent_session_uuid, b.turn_id, b.score
FROM best b JOIN catalog p ON p.id = b.plan_id
LEFT JOIN plan_phases ph ON ph.id = b.phase_id
ORDER BY b.priority DESC, b.score DESC, p.updated_at DESC, p.id
LIMIT $9
