-- Give the control plane an identity of its own so a node can tell the real
-- control plane from anything else that answers on the same address.
--
-- Until now the node handshake was one-way: control learned who the node was,
-- but nothing proved the peer was control. A node pins this key on its first
-- successful pairing and refuses every later connection that cannot sign for
-- it. The key lives here rather than in a host file so it survives redeploys
-- without a new dataset; anything that can read it can already read the
-- credentials control hands out.

CREATE TABLE control_identity (
    id SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    credential_kind TEXT NOT NULL DEFAULT 'ed25519'
        CHECK (credential_kind = 'ed25519'),
    private_key BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- The control plane's WireGuard identity, owned by the tunnel sidecar. The
-- sidecar generates it and holds the private half; the control process reads
-- only the public half, to hand to nodes as part of their peering.
CREATE TABLE control_tunnel (
    id SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    private_key BYTEA NOT NULL CHECK (octet_length(private_key) = 32),
    public_key BYTEA NOT NULL CHECK (octet_length(public_key) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- The WireGuard peering a node is granted when it is approved. The public key
-- is offered during the cleartext enrollment hop; the tunnel address is
-- assigned by control. Credentials are only delivered once the connection
-- arrives over the resulting tunnel.
ALTER TABLE dev_nodes
    ADD COLUMN pending_tunnel_public_key BYTEA,
    ADD COLUMN tunnel_public_key BYTEA,
    -- Text rather than INET: the address is only ever rendered back into a
    -- WireGuard config, and this avoids a network-type dependency for a value
    -- control itself allocates.
    ADD COLUMN tunnel_address TEXT;

ALTER TABLE dev_nodes
    ADD CONSTRAINT dev_nodes_tunnel_key_length_check
    CHECK (
        (pending_tunnel_public_key IS NULL
            OR octet_length(pending_tunnel_public_key) = 32)
        AND
        (tunnel_public_key IS NULL OR octet_length(tunnel_public_key) = 32)
    );

CREATE UNIQUE INDEX dev_nodes_tunnel_address_idx
    ON dev_nodes (tunnel_address)
    WHERE tunnel_address IS NOT NULL;
