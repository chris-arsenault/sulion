-- Grants record their scope explicitly. `all_terminals` grants apply to every
-- registered credential, but only for the programs they list: redemption
-- injects one when the signed program name is in `programs`. They rank below
-- terminal and repository grants, which replace their value for the same
-- variable.
ALTER TABLE secret_broker.grants
    ADD COLUMN scope TEXT,
    ADD COLUMN programs TEXT[];

UPDATE secret_broker.grants
   SET scope = CASE WHEN pty_session_id IS NOT NULL THEN 'terminal' ELSE 'repository' END;

ALTER TABLE secret_broker.grants ALTER COLUMN scope SET NOT NULL;

ALTER TABLE secret_broker.grants DROP CONSTRAINT grant_scope;
ALTER TABLE secret_broker.grants ADD CONSTRAINT grant_scope CHECK (
    (scope = 'terminal' AND pty_session_id IS NOT NULL AND repo IS NULL
        AND expires_at IS NOT NULL AND programs IS NULL)
    OR (scope = 'repository' AND pty_session_id IS NULL AND length(trim(repo)) > 0
        AND expires_at IS NULL AND programs IS NULL)
    OR (scope = 'all_terminals' AND pty_session_id IS NULL AND repo IS NULL
        AND expires_at IS NULL AND cardinality(programs) > 0)
);

CREATE UNIQUE INDEX secret_broker_all_terminals_grant_active_idx
  ON secret_broker.grants (secret_id)
  WHERE scope = 'all_terminals' AND revoked_at IS NULL;

DROP VIEW secret_broker.effective_grants;
CREATE VIEW secret_broker.effective_grants AS
  SELECT COALESCE(g.pty_session_id, p.pty_session_id) AS pty_session_id,
         g.secret_id, g.granted_by_sub, g.granted_by_username, g.expires_at,
         g.repo, g.scope, g.programs,
         CASE g.scope WHEN 'all_terminals' THEN 0 ELSE 1 END AS precedence
  FROM secret_broker.grants g
  LEFT JOIN secret_broker.pty_credentials p
    ON p.revoked_at IS NULL
   AND ((g.scope = 'repository' AND p.repo = g.repo) OR g.scope = 'all_terminals')
  WHERE g.revoked_at IS NULL
    AND (g.scope <> 'terminal' OR g.expires_at > NOW())
    AND (g.scope = 'terminal' OR p.pty_session_id IS NOT NULL);

-- Redemption has one path, bare with-cred, so the per-use tool name is gone.
ALTER TABLE secret_broker.use_audit DROP COLUMN tool;
