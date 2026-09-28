ALTER TABLE secret_broker.pty_credentials ADD COLUMN repo TEXT;

ALTER TABLE secret_broker.grants
  ALTER COLUMN pty_session_id DROP NOT NULL,
  ALTER COLUMN expires_at DROP NOT NULL,
  ADD COLUMN repo TEXT,
  ADD CONSTRAINT grant_scope CHECK (
    (pty_session_id IS NOT NULL AND repo IS NULL AND expires_at IS NOT NULL)
    OR (pty_session_id IS NULL AND repo IS NOT NULL AND length(trim(repo)) > 0 AND expires_at IS NULL)
  );

CREATE UNIQUE INDEX secret_broker_repository_grant_active_idx
  ON secret_broker.grants (repo, secret_id)
  WHERE repo IS NOT NULL AND revoked_at IS NULL;

CREATE VIEW secret_broker.effective_grants AS
  SELECT pty_session_id, secret_id, granted_by_sub, granted_by_username,
         expires_at, repo
  FROM secret_broker.grants
  WHERE revoked_at IS NULL AND expires_at > NOW()
  UNION ALL
  SELECT p.pty_session_id, g.secret_id, g.granted_by_sub, g.granted_by_username,
         g.expires_at, g.repo
  FROM secret_broker.grants g
  JOIN secret_broker.pty_credentials p ON p.repo = g.repo AND p.revoked_at IS NULL
  WHERE g.revoked_at IS NULL AND g.expires_at IS NULL;
