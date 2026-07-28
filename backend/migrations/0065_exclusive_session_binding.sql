-- An agent session lives in exactly one PTY. Correlation now releases a
-- session from its previous PTY when it moves, but rows bound before that fix
-- can still share one session across two PTYs — which the sidebar renders as
-- the same conversation twice. Keep the binding claude_sessions considers
-- authoritative and release the rest.

UPDATE pty_sessions ps
   SET current_session_uuid = NULL,
       current_session_agent = NULL,
       current_claude_session_uuid = NULL
 WHERE ps.current_session_uuid IS NOT NULL
   AND EXISTS (
        SELECT 1
          FROM claude_sessions cs
         WHERE cs.session_uuid = ps.current_session_uuid
           AND cs.pty_session_id IS NOT NULL
           AND cs.pty_session_id <> ps.id
       );
