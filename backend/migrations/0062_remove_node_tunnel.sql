-- Remove the WireGuard tunnel state.
--
-- The tunnel's control end needed CAP_NET_ADMIN and the wireguard kernel
-- module on the TrueNAS host, which contradicts the reason dedicated nodes
-- exist: the control host grants no elevated privileges. The node channel
-- stays LAN-confined and mutually authenticated (control_identity remains);
-- transport encryption, if added later, must terminate in userspace.

ALTER TABLE dev_nodes
    DROP CONSTRAINT IF EXISTS dev_nodes_tunnel_key_length_check;

DROP INDEX IF EXISTS dev_nodes_tunnel_address_idx;

ALTER TABLE dev_nodes
    DROP COLUMN IF EXISTS pending_tunnel_public_key,
    DROP COLUMN IF EXISTS tunnel_public_key,
    DROP COLUMN IF EXISTS tunnel_address;

DROP TABLE IF EXISTS control_tunnel;
