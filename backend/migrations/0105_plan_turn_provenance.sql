CREATE INDEX plan_attachments_plan_history_idx ON plan_attachments(plan_id, attached_at);
CREATE INDEX plan_attachments_session_detached_idx
    ON plan_attachments(agent_session_uuid, detached_at) WHERE detached_at IS NOT NULL;

-- Resolve against the retained timeline so links work for old plans and for
-- turns projected after a plan command finishes. Turn identity is session-local.
CREATE FUNCTION plan_turn_provenance(target_plan UUID)
RETURNS TABLE (phase_id UUID, session_uuid UUID, turn_id BIGINT, kind TEXT, event_id BIGINT)
LANGUAGE SQL STABLE AS $$
    WITH plan AS (
        SELECT * FROM plans WHERE id = target_plan
    ), audit AS (
        SELECT * FROM plan_events WHERE plan_id = target_plan
    ), event_turns AS (
        SELECT e.id, e.phase_id, e.authoring_change, e.agent_session_uuid, turn.turn_id
        FROM audit e
        JOIN LATERAL (
            SELECT t.turn_id
            FROM timeline_turns t
            WHERE t.session_uuid = e.agent_session_uuid AND NOT t.is_sidechain_turn
              AND t.start_timestamp <= e.created_at AND t.end_timestamp >= e.created_at
            ORDER BY t.start_timestamp DESC, t.turn_id DESC
            LIMIT 1
        ) turn ON TRUE
    ), phase_windows AS (
        SELECT e.phase_id, e.to_status, e.created_at AS started,
               LEAD(e.created_at) OVER (PARTITION BY e.phase_id ORDER BY e.created_at, e.id) AS ended
        FROM audit e
        WHERE e.phase_id IS NOT NULL AND e.to_status IS NOT NULL
    ), plan_windows AS (
        SELECT e.to_status, e.created_at AS started,
               LEAD(e.created_at) OVER (ORDER BY e.created_at, e.id) AS ended
        FROM audit e
        WHERE e.event_type IN ('plan_created', 'branch_created', 'plan_status_changed', 'plan_closed')
    ), attached_turns AS (
        SELECT DISTINCT t.session_uuid, t.turn_id, t.start_timestamp
        FROM plan_attachments a
        JOIN pty_sessions pty ON pty.id = a.pty_session_id
        JOIN claude_sessions s ON s.session_uuid = a.agent_session_uuid
                            OR (s.pty_session_id = a.pty_session_id
                                AND (a.agent_session_uuid IS NULL OR s.started_at >= a.attached_at))
        JOIN plan_windows w ON w.to_status = 'active'
        JOIN timeline_turns t ON t.session_uuid = s.session_uuid AND NOT t.is_sidechain_turn
          AND t.start_timestamp >= GREATEST(a.attached_at, w.started)
          AND t.start_timestamp < COALESCE(LEAST(a.detached_at, w.ended, pty.ended_at), 'infinity')
        WHERE a.plan_id = target_plan
    ), attached_sources AS (
        SELECT t.session_uuid, t.turn_id, (array_agg(w.phase_id))[1] AS possible_phase,
               count(w.phase_id) AS phase_count
        FROM attached_turns t
        LEFT JOIN phase_windows w ON w.to_status = 'in_progress'
          AND t.start_timestamp >= w.started AND t.start_timestamp < COALESCE(w.ended, 'infinity')
        GROUP BY t.session_uuid, t.turn_id
    ), draft_turns AS (
        SELECT t.session_uuid, t.turn_id
        FROM plan p
        LEFT JOIN LATERAL (
            SELECT MAX(a.detached_at) AS previous_end
            FROM plan_attachments a
            WHERE (a.agent_session_uuid = p.created_by_agent_session_uuid
                OR (a.agent_session_uuid IS NULL AND a.pty_session_id = p.created_by_pty_id))
              AND a.plan_id <> target_plan
              AND a.attached_at < p.created_at
              AND a.detached_at <= p.created_at
        ) previous ON TRUE
        JOIN timeline_turns t ON t.session_uuid = p.created_by_agent_session_uuid
          AND NOT t.is_sidechain_turn AND t.start_timestamp < p.created_at
          AND t.start_timestamp >= COALESCE(previous.previous_end, '-infinity')
        WHERE p.parent_plan_id IS NULL
    )
    SELECT e.phase_id, e.agent_session_uuid, e.turn_id, 'change', e.id
    FROM event_turns e
    UNION
    SELECT ph.id, e.agent_session_uuid, e.turn_id, 'change', e.id
    FROM event_turns e
    CROSS JOIN LATERAL jsonb_array_elements(e.authoring_change->'changes') op
    JOIN plan_phases ph ON ph.plan_id = target_plan AND ph.id::text = op->>'id'
    WHERE op->>'op' IN ('edit_phase', 'add_phase', 'move_phase', 'retire_phase', 'record_evidence')
    UNION
    SELECT CASE WHEN a.phase_count = 1 THEN a.possible_phase END,
           a.session_uuid, a.turn_id,
           CASE WHEN a.phase_count > 0 THEN 'execution' ELSE 'planning_context' END,
           NULL::BIGINT
    FROM attached_sources a
    UNION
    SELECT NULL::UUID, t.session_uuid, t.turn_id, 'planning_context', NULL::BIGINT
    FROM draft_turns t
$$;
