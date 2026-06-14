-- Device pairing + tokens for external tools (first consumer: the Ableton
-- "Send to Sulion" extension). Pairing follows the OAuth device-authorization
-- shape: a device gets a short user_code, the logged-in user approves it in the
-- browser, and the device exchanges its device_code for a long-lived opaque
-- token. Only base64(SHA-256) hashes of secrets are stored.
--
-- The minted token then authenticates generic content writes via
-- POST /api/repos/:name/ingest, which writes bytes to a file on disk under the
-- repo (like the terminal's paste-as-file) — there is no content table.

-- In-flight pairing requests. `device_code` and the minted token are never
-- persisted in the clear — only their base64(SHA-256) hashes.
CREATE TABLE IF NOT EXISTS device_pairings (
    id BIGSERIAL PRIMARY KEY,
    device_code_hash TEXT NOT NULL UNIQUE,
    user_code TEXT NOT NULL UNIQUE,
    client TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'claimed', 'expired', 'denied')),
    -- Set when a logged-in user approves; binds the eventual token to them.
    user_sub TEXT,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Lookup by the human code while approving (only pending rows matter).
CREATE INDEX IF NOT EXISTS device_pairings_user_code_pending_idx
    ON device_pairings(user_code)
    WHERE status = 'pending';

-- Long-lived device tokens minted once a pairing is claimed. Revocable via
-- revoked_at; never expiring otherwise.
CREATE TABLE IF NOT EXISTS device_tokens (
    id BIGSERIAL PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    user_sub TEXT NOT NULL,
    client TEXT NOT NULL,
    label TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS device_tokens_active_idx
    ON device_tokens(user_sub)
    WHERE revoked_at IS NULL;
