-- Development-node identity, ownership, and idempotent operation state.
--
-- All ownership columns are nullable during the standalone-to-split migration:
-- NULL means the resource is still owned by the legacy in-process runtime.

CREATE TABLE dev_nodes (
    id UUID PRIMARY KEY,
    display_name TEXT NOT NULL,
    credential_kind TEXT NOT NULL DEFAULT 'ed25519'
        CHECK (credential_kind IN ('ed25519', 'internal')),
    public_key BYTEA,
    credential_fingerprint TEXT,
    credential_generation INTEGER NOT NULL DEFAULT 1
        CHECK (credential_generation > 0),
    protocol_version INTEGER,
    control_protocol_min INTEGER,
    control_protocol_max INTEGER,
    build_git_sha TEXT,
    capabilities JSONB NOT NULL DEFAULT '[]'::jsonb,
    docker_policy TEXT NOT NULL DEFAULT 'none'
        CHECK (docker_policy IN ('none', 'brokered', 'direct')),
    docker_info JSONB NOT NULL DEFAULT '{}'::jsonb,
    path_contract_version INTEGER,
    boot_id UUID,
    connection_id UUID,
    connection_state TEXT NOT NULL DEFAULT 'enrolled'
        CHECK (connection_state IN (
            'enrolled', 'connected', 'disconnected', 'stale',
            'incompatible', 'revoked'
        )),
    compatibility_error TEXT,
    desired_release_digest TEXT,
    observed_release_digest TEXT,
    drain_state TEXT NOT NULL DEFAULT 'accepting'
        CHECK (drain_state IN ('accepting', 'draining', 'drained')),
    enrolled_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    connected_at TIMESTAMPTZ,
    last_heartbeat_at TIMESTAMPTZ,
    node_disconnected_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at TIMESTAMPTZ,
    CHECK (
        (credential_kind = 'internal' AND public_key IS NULL)
        OR
        (credential_kind = 'ed25519' AND octet_length(public_key) = 32)
    )
);

CREATE UNIQUE INDEX dev_nodes_credential_fingerprint_uidx
    ON dev_nodes(credential_fingerprint)
    WHERE credential_fingerprint IS NOT NULL AND revoked_at IS NULL;

CREATE INDEX dev_nodes_connection_state_idx
    ON dev_nodes(connection_state);

CREATE TABLE dev_node_credentials (
    node_id UUID NOT NULL REFERENCES dev_nodes(id) ON DELETE CASCADE,
    generation INTEGER NOT NULL CHECK (generation > 0),
    public_key BYTEA NOT NULL CHECK (octet_length(public_key) = 32),
    credential_fingerprint TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    replaced_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (node_id, generation)
);

CREATE TABLE dev_node_enrollment_tokens (
    id UUID PRIMARY KEY,
    token_hash BYTEA NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    target_node_id UUID REFERENCES dev_nodes(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (expires_at > created_at)
);

CREATE INDEX dev_node_enrollment_tokens_expiry_idx
    ON dev_node_enrollment_tokens(expires_at)
    WHERE used_at IS NULL;

CREATE TABLE dev_node_boots (
    node_id UUID NOT NULL REFERENCES dev_nodes(id) ON DELETE CASCADE,
    boot_id UUID NOT NULL,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    disconnected_at TIMESTAMPTZ,
    build_git_sha TEXT,
    protocol_version INTEGER NOT NULL,
    observed_release_digest TEXT,
    PRIMARY KEY (node_id, boot_id)
);

CREATE TABLE dev_node_operations (
    operation_id UUID PRIMARY KEY,
    idempotency_key TEXT NOT NULL,
    node_id UUID NOT NULL REFERENCES dev_nodes(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL,
    resource_id UUID,
    request_payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    dispatched_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'dispatched', 'succeeded', 'failed', 'canceled')),
    result JSONB,
    error_code TEXT,
    error_message TEXT,
    dispatch_boot_id UUID,
    dispatch_count INTEGER NOT NULL DEFAULT 0 CHECK (dispatch_count >= 0),
    UNIQUE (node_id, idempotency_key),
    CHECK (
        (status IN ('succeeded', 'failed', 'canceled') AND completed_at IS NOT NULL)
        OR
        (status IN ('pending', 'dispatched') AND completed_at IS NULL)
    )
);

CREATE INDEX dev_node_operations_dispatch_idx
    ON dev_node_operations(node_id, requested_at)
    WHERE status IN ('pending', 'dispatched');

ALTER TABLE pty_sessions
    ADD COLUMN node_id UUID REFERENCES dev_nodes(id) ON DELETE RESTRICT,
    ADD COLUMN node_boot_id UUID,
    ADD COLUMN control_disconnected_at TIMESTAMPTZ,
    ADD COLUMN node_disconnected_at TIMESTAMPTZ,
    ADD COLUMN runtime_end_reason TEXT,
    ADD COLUMN deleted_at TIMESTAMPTZ;

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
