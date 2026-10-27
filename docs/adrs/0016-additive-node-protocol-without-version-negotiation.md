# ADR 0016: Additive Node Protocol Without Version Negotiation

## Status

Accepted, 2026-07-29.

## Context

The control plane is deployed by Komodo on a push to `main`. The dedicated
NixOS node polls the `node-release` branch on its own root-owned timer. Both
run the same image, but they are released independently with an arbitrary
gap, and either side may be ahead of the other at any moment.

ADR 0002 as first written said control and node "accept one exact protocol
version and are upgraded together." The protocol constant
`NODE_PROTOCOL_VERSION` has been `1` since `956e210`. The first field added
after the split, a whole-machine memory and CPU sample in the heartbeat,
forced the question: does a new optional field move the version, and if the
version moves, how do two releases that are never deployed simultaneously
keep talking?

A review draft proposed replacing the exact-version check with range
negotiation, written against a working-tree bump to version 2. That bump was
reverted before it reached `main`.

## Decision

Node protocol changes are additive and absent-tolerant. New payload fields
are optional, a reader treats an absent field as "not reported" rather than
an error, and no message uses `deny_unknown_fields`, so a peer from either
release parses the other's payloads. Adding a field is never a reason to
change `NODE_PROTOCOL_VERSION`.

Control and node continue to check one exact version, and that version stays
at `1`. A bump is reserved for a message that genuinely cannot be served by
both builds, and such a change must carry its own compatibility plan. New
request kinds ride the same rule: an older node answers an unknown kind with
"unsupported" and the caller handles that result.

The protocol deliberately does not implement capability negotiation, rolling
protocol ranges, or release reconciliation. The same rule applies to every
other contract that spans the two release timers, such as the correlate
socket payload and persisted columns an older instance may still write: those
need release sequencing in two phases, not negotiation.

## Alternatives considered

- **Range or capability negotiation at handshake.** Proposed by the July 2026
  cleanup review and recorded there as "not pursued." Under the additive rule the exact-version checks never fire,
  so negotiation would be machinery for a case the policy prevents. The same
  conclusion was reached again during the meta-repository work, where a
  heartbeat capability map had been added solely to preflight one request
  kind; the review found the protocol's existing "unsupported" response
  already covered it, and `docs/meta-repositories.md` now states that no
  heartbeat feature negotiation is needed.
- **Bump the version for each protocol change.** The reverted v2 bump. A bump
  severs every peer that has not upgraded until both sides restart, which with
  independent release timers means a guaranteed outage window on every
  protocol-touching release. `3886c86` replaced the "upgraded together"
  wording in ADR 0002 and `docs/node-protocol.md` for this reason.
- **Ordered deployment.** Requiring control before node, or the reverse, was
  what the "upgraded together" wording implied. `3886c86` notes it would have
  made every release deploy-order dependent, and the cleanup plan records the
  deploy-ordering constraint as applying throughout: nothing in the repo may
  assume which side ships first.
- **Strip the absent-tolerant defaults as cruft.** An earlier review pass
  flagged `#[serde(default)]` on `node_nonce` and `inventory_complete` for
  removal on the assumption that strict version equality had already severed
  old peers. The cleanup plan reverses that: those defaults are the mechanism
  that makes either deploy order work and are kept permanently.

## Consequences

- Every new node-protocol field, request kind, and store change is written
  with a default and tested with it absent. Later work followed this shape:
  the devenv session-upgrade request (`6cd80c3`) and the meta-repository
  launch fields (`fb5fc58`) both rode the protocol additively.
- A newer side's optional data is simply missing until both have shipped;
  the UI must tolerate a node that does not yet report a field.
- Removing a field or alias is a two-release operation: stop sending in one
  release, stop accepting in a later one.
- A protocol change that cannot be expressed additively is an explicit,
  planned event with its own compatibility plan, not a routine commit.
- The protocol stays small: one version constant, four persisted connection
  states, and no negotiation state to maintain.

## References

- Compatibility rule: [`../node-protocol.md`](../node-protocol.md),
  "Stability choices"
- The July 2026 cleanup review's protocol-compatibility chunk, which
  proposed and then withdrew range negotiation, is folded into this record
- Topology decision this refines:
  [ADR 0002](0002-hybrid-control-plane-and-dev-node.md), "Node boundary"
- Commits: `956e210` (version constant introduced at 1), `3886c86`
  (additive rule written into ADR 0002 and node-protocol.md), `80f66a8`
  (every deployment routed through the one protocol)
- CHANGELOG v2.0.0, "Changed the node protocol to additive-only
  compatibility"
