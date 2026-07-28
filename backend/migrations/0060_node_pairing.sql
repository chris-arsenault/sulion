-- Replace copied enrollment tokens with one-click approval of the fixed
-- dedicated node's outbound, proof-of-possession handshake.

ALTER TABLE dev_nodes
    ADD COLUMN pending_public_key BYTEA;

ALTER TABLE dev_nodes
    ALTER COLUMN enrolled_at DROP NOT NULL;

ALTER TABLE dev_nodes
    DROP CONSTRAINT dev_nodes_connection_state_check;

ALTER TABLE dev_nodes
    ADD CONSTRAINT dev_nodes_connection_state_check
    CHECK (connection_state IN ('pending', 'enrolled', 'connected', 'disconnected'));

ALTER TABLE dev_nodes
    ADD CONSTRAINT dev_nodes_pending_public_key_check
    CHECK (
        (credential_kind = 'internal' AND pending_public_key IS NULL)
        OR
        (credential_kind = 'ed25519' AND (
            pending_public_key IS NULL OR octet_length(pending_public_key) = 32
        ))
    );

UPDATE dev_nodes
SET connection_state = 'pending',
    enrolled_at = NULL,
    updated_at = NOW()
WHERE credential_kind = 'ed25519'
  AND public_key IS NULL;

DROP TABLE dev_node_enrollment_tokens;
