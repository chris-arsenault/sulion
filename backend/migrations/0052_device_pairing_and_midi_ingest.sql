-- Device pairing + MIDI clip ingest for external extensions (first consumer:
-- the Ableton "Send to Sulion" extension). Pairing follows the OAuth
-- device-authorization shape: a device gets a short user_code, the logged-in
-- user approves it in the browser, and the device exchanges its device_code for
-- a long-lived opaque token. Only hashes of secrets are stored.

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

-- Captured MIDI clips. Notes are stored verbatim as JSONB; `note_count` is
-- denormalized for cheap listing. ingest_id is the app-facing handle.
CREATE TABLE IF NOT EXISTS midi_clips (
    id BIGSERIAL PRIMARY KEY,
    ingest_id UUID NOT NULL UNIQUE,
    device_token_id BIGINT REFERENCES device_tokens(id) ON DELETE SET NULL,
    user_sub TEXT NOT NULL,
    source TEXT NOT NULL,
    name TEXT,
    tempo DOUBLE PRECISION,
    length_beats DOUBLE PRECISION,
    time_sig_numerator INT,
    time_sig_denominator INT,
    note_count INT NOT NULL,
    notes JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS midi_clips_user_recent_idx
    ON midi_clips(user_sub, created_at DESC);
