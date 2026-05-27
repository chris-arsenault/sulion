DROP INDEX IF EXISTS secret_broker.secret_broker_grants_active_idx;

ALTER TABLE secret_broker.grants
  DROP COLUMN IF EXISTS tool;

CREATE INDEX IF NOT EXISTS secret_broker_grants_active_idx
  ON secret_broker.grants (pty_session_id, secret_id, expires_at)
  WHERE revoked_at IS NULL;
