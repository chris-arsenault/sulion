CREATE TABLE IF NOT EXISTS secret_broker.pty_credentials (
  pty_session_id UUID PRIMARY KEY,
  public_key TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  revoked_at TIMESTAMPTZ NULL
);

CREATE TABLE IF NOT EXISTS secret_broker.pty_use_nonces (
  pty_session_id UUID NOT NULL,
  nonce TEXT NOT NULL,
  seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY (pty_session_id, nonce)
);

CREATE INDEX IF NOT EXISTS secret_broker_pty_use_nonces_seen_idx
  ON secret_broker.pty_use_nonces (seen_at);
