# ADR 0015: Userspace TLS Node Endpoint Instead of WireGuard

## Status

Accepted, 2026-07-28.

## Context

ADR 0002 split Sulion into a TrueNAS control plane and a dedicated
development node that dials out to it. The first shipped bootstrap
(`9eef02e`, 2026-07-27) encrypted everything after enrollment inside a
WireGuard tunnel whose control end terminated in the control process's own
network namespace. The node channel carries database credentials, the
retrieval token, the broker registration token, and terminal bytes, so
encryption on the LAN was never optional.

Deploying that design on TrueNAS showed that the tunnel's control end needed
`CAP_NET_ADMIN` and the `wireguard` kernel module on the host. The dedicated
node exists precisely so the shared control host grants no elevated
privileges; a kernel tunnel in the control stack contradicted that premise.
The tunnel also brought a sidecar container, its binary, a peering exchange,
root-owned `wg` host units, and a tunnel schema.

The broker and retrieval services were separate node destinations as well,
each needing its own transport decision.

## Decision

All node traffic rides one LAN-bound endpoint, `192.168.66.3:30081`, where
the control process itself terminates TLS with a certificate it generates and
stores beside its control identity. The control channel is `wss://…/ws/nodes`;
an in-process gateway proxies `/broker` and `/retrieval` on the same port to
the sibling containers over the host-internal Docker network. Insecure `ws://`
is refused in deployment configuration.

Trust is pinned rather than CA-issued, and bound to the approved node
identity:

- the node records the certificate at first pairing and refuses any other at
  the TLS layer, before a protocol byte flows;
- the signed Ed25519 handshake proof carries the certificate digest, so a
  substituted endpoint cannot present a valid proof even on first contact;
- PTY tools trust the pinned certificate as an extra root through a
  world-readable runtime copy, leaving public TLS untouched.

The published node port targets a listener that serves only the node router
and the gateway. The frontend returns 404 for `/ws/nodes`, no reverse-proxy
upstream references the port, the control plane refuses node sockets from
outside `SULION_NODE_LAN_CIDR`, and `make validate-deploy` fails if any node
destination points at the public hostname. The compose validation also
asserts that no tunnel service exists, so a reintroduced tunnel fails the
build (`6c70ddd`).

## Alternatives considered

- **WireGuard tunnel between node and control process.** Shipped in
  `9eef02e` and removed in `1330699`. The control end required
  `CAP_NET_ADMIN` and a kernel module on the TrueNAS host, which the control
  host does not grant, and it added a sidecar, a peering exchange, root
  systemd units on the node, and schema that only existed to track tunnel
  state (dropped by migration 0062).
- **CA-based TLS on the node port.** `1330699` states the choice directly:
  trust is pinned, not CA-based. A CA would reintroduce a public-certificate
  dependency for a LAN-only path, and would not bind the certificate to the
  Ed25519 identity an operator approved. Pinning plus the digest in the
  handshake proof makes the endpoint part of the approved identity; recovery
  from a reissued certificate is the same deliberate re-approval as the
  identity pin.
- **Plaintext WebSocket on the LAN.** Rejected by requirement: the channel
  carries credentials, and `1330699` makes insecure `ws://` a configuration
  error. `docs/node-protocol.md` records that everything on the node port is
  encrypted.
- **Separate transports for broker and retrieval.** Rejected in favor of one
  endpoint, one certificate, one pin. The gateway keeps the sibling
  containers reachable without exposing them on the LAN individually.
- **Browsers connecting directly to the node.** Already rejected in
  [ADR 0002](0002-hybrid-control-plane-and-dev-node.md#connect-browsers-directly-to-the-node);
  this decision keeps the node outbound-only and the browser on a single
  origin.

## Consequences

- The backend API is no longer reachable on the LAN; only the node router and
  gateway listen on the published port.
- The control container runs with no added capabilities, and the NixOS host
  carries no WireGuard units for Sulion.
- A legitimately reissued control certificate requires deleting both pin
  files on the node and approving it again.
- Node, broker, and retrieval clients inside PTYs must carry
  `SULION_CONTROL_TLS_CA`; the node runtime publishes the pinned certificate
  copy for them.
- Public ingress for browsers is unchanged and still passes through the
  Ahara reverse proxy and its own WireGuard ingress (`646e488`); that path is
  separate from the node channel and stays out of this decision.

## References

- Transport and trust contract: [`../node-protocol.md`](../node-protocol.md)
- Deployment ports and LAN boundary: [`../deploy.md`](../deploy.md)
- Topology decision this refines:
  [ADR 0002](0002-hybrid-control-plane-and-dev-node.md)
- Commits: `9eef02e` (WireGuard bootstrap), `1330699` (userspace TLS
  replacement), `6c70ddd` (tunnel-absence assertion recorded as invariant),
  `646e488` (public Ahara VPN exposure)
- CHANGELOG v2.0.0, "Dedicated development node"
