-- One development-node identity and resource ownership for the
-- standalone-to-split migration. NULL resource ownership retains the
-- existing in-process standalone runtime.

CREATE TABLE dev_nodes (
    id UUID PRIMARY KEY,
    display_name TEXT NOT NULL,
    credential_kind TEXT NOT NULL DEFAULT 'ed25519'
        CHECK (credential_kind IN ('ed25519', 'internal')),
    public_key BYTEA,
    protocol_version INTEGER,
    boot_id UUID,
    connection_id UUID,
    connection_state TEXT NOT NULL DEFAULT 'enrolled'
        CHECK (connection_state IN ('enrolled', 'connected', 'disconnected')),
    enrolled_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    connected_at TIMESTAMPTZ,
    last_heartbeat_at TIMESTAMPTZ,
    node_disconnected_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (
        (credential_kind = 'internal' AND public_key IS NULL)
        OR
        (credential_kind = 'ed25519' AND (
            public_key IS NULL OR octet_length(public_key) = 32
        ))
    )
);

CREATE TABLE dev_node_enrollment_tokens (
    id UUID PRIMARY KEY,
    token_hash BYTEA NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    target_node_id UUID NOT NULL REFERENCES dev_nodes(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (expires_at > created_at)
);

CREATE INDEX dev_node_enrollment_tokens_expiry_idx
    ON dev_node_enrollment_tokens(expires_at)
    WHERE used_at IS NULL;

ALTER TABLE pty_sessions
    ADD COLUMN node_id UUID REFERENCES dev_nodes(id) ON DELETE RESTRICT,
    ADD COLUMN node_boot_id UUID,
    ADD COLUMN node_disconnected_at TIMESTAMPTZ,
    ADD COLUMN runtime_end_reason TEXT;

CREATE INDEX pty_sessions_node_live_idx
    ON pty_sessions(node_id, node_boot_id)
    WHERE state = 'live' AND node_id IS NOT NULL;

ALTER TABLE workspaces
    ADD COLUMN node_id UUID REFERENCES dev_nodes(id) ON DELETE RESTRICT;

CREATE INDEX workspaces_node_idx
    ON workspaces(node_id)
    WHERE node_id IS NOT NULL AND state <> 'deleted';

ALTER TABLE repos
    ADD COLUMN node_id UUID REFERENCES dev_nodes(id) ON DELETE RESTRICT;

CREATE INDEX repos_node_idx
    ON repos(node_id)
    WHERE node_id IS NOT NULL;

ALTER TABLE code_roots
    ADD COLUMN node_id UUID REFERENCES dev_nodes(id) ON DELETE RESTRICT;

CREATE INDEX code_roots_node_idx
    ON code_roots(node_id)
    WHERE node_id IS NOT NULL AND deleted_at IS NULL;
