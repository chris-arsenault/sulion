-- 0065 released duplicate session bindings only when claude_sessions named an
-- authoritative PTY. Ingester-discovered sessions have no such link, so their
-- duplicates survived, and nodes still running the pre-fix correlate binary
-- kept minting new ones. Release duplicates unconditionally: for every session
-- claimed by more than one PTY, keep the most recently created claimant and
-- release the rest.

UPDATE pty_sessions ps
   SET current_session_uuid = NULL,
       current_session_agent = NULL,
       current_claude_session_uuid = NULL
 WHERE ps.current_session_uuid IS NOT NULL
   AND ps.id <> (
        SELECT keep.id
          FROM pty_sessions keep
         WHERE keep.current_session_uuid = ps.current_session_uuid
         ORDER BY keep.created_at DESC, keep.id DESC
         LIMIT 1
       );
