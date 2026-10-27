# ADR 0010: Cognito Browser Authentication, Fail-Closed Outages, and a Broker-Held Revocation Cutoff

## Status

Accepted, 2026-10-14.

## Context

Commit e5e8ab4 (2026-04-22) put the browser behind the shared Ahara Cognito
pool: the frontend blocks on sign-in, the backend requires a valid Sulion app
token on API and WebSocket routes, and project Terraform creates the app
client. Token validation was offline: signature, issuer, expiry.

The external audit of 2026-09-28 found the gaps in that shape. F01: a
device-pairing flow issued permanent, broad device tokens that had never been
used. F03: no server-side revocation existed; an attached WebSocket kept
working after its JWT expired, and sign-out was a local SDK action. F06: an
absent `SULION_AUTH_ISSUER_URL` made `require_http_auth` insert a synthetic
dev principal with no production guard. F07: JWKS refresh ran on every
unknown key with no shared in-flight request, cooldown, or timeout, so an
identity-service outage amplified into every expired-cache request.

Commit 3d40436 (2026-09-28) landed the remediation. Two browser defects
followed. On 2026-10-13 the user reported being stuck in an unauthenticated
state where requests rejected but the UI never returned to login (session
`01a13618-5d3b-71a3-91c3-8f4f53d73b76`, turn 106212). After that fix, the
opposite failure appeared: the shared fetch boundary sent every 401 to
`AuthProvider`, which cleared the login, so a Cognito, broker, or database
blip signed the user out.

## Decision

Sulion accepts Cognito access tokens only, issued for five minutes, and
validates signature, issuer, client, `exp`, `iat`, `auth_time`, and `sub`
independently at the ALB and in the backend. Production roles and the broker
refuse to start without issuer and client settings; a test bypass is an
explicit development choice that production configuration rejects. Worker
binaries (node, ingester) load worker configuration without browser auth.

The broker owns a durable per-principal `revoked_before` cutoff
(`secret_broker.browser_revocations`). Tokens whose Cognito `auth_time` is at
or before the cutoff lose Sulion access, including tokens refreshed from
those logins, because refresh keeps the original `auth_time`. REST, broker
management, and ticket issuance check the cutoff per request; open sockets
recheck every five seconds with a three-second deadline and close on failure
without killing the PTY. **Sign out (this device)** clears local credentials;
**Sign out (all devices)** writes the cutoff first. An unauthenticated or
already-revoked request cannot advance the cutoff.

Outages are distinguished from rejection. A confirmed expiry or revocation is
401 and returns the browser to login. Database errors, bounded query
timeouts, broker connection failures, and missing signing keys return 503
with `Retry-After: 30`, and the browser keeps its credentials and retries
with backoff. Signing keys are refreshed in the background every 15 minutes;
each successful key set stays usable for 60 minutes from its fetch, failed
refreshes keep the previous set without extending it, and refreshes for
unknown key IDs are coalesced and limited to one per 30 seconds. Only an
explicit 401 is replayed, once, with a refreshed token.

Device tokens and pairing are deleted outright: routes, UI, and tables
(migration 0097), with no compatibility window.

## Alternatives considered

- **Long-lived device tokens.** Shipped for the Ableton file contract and
  never used. The user directed complete removal rather than hardening, and
  Phase 5 of the remediation plan states "Do not create a replacement
  device-token system." 3d40436 deleted `device_routes.rs`, `PairPage`, and
  the token tables.
- **JWT-only validation with no server-side revocation.** The state from
  e5e8ab4 to 3d40436. F03 showed an attached client outliving its token and
  sign-out that revoked nothing; the Cognito pre-authentication trigger does
  not run on refresh, so removing an application grant alone could not cover
  it. The plan's recommended mechanism was adopted: short token lifetime,
  tickets carrying principal and expiry, and a durable revoke-before
  timestamp checked by REST, broker, and live attachments.
- **Treating every authentication failure as a sign-out.** The pre-521c20b
  boundary sent any 401 to `AuthProvider`, which cleared the login. e180edb
  reversed the outage half: "Return 503 when keys are temporarily unavailable,
  keeping browser credentials intact while preserving invalid-token
  rejection." The same release kept confirmed rejections at 401 so the
  2026-10-13 stuck state does not return.
- **Unbounded JWKS refresh, or accepting stale keys indefinitely.** F07
  rejected the first; Phase 4 rejected the second ("Do not allow unknown keys
  or indefinitely extend stale-key acceptance"). The 15-minute background
  refresh with a 60-minute retention and a 30-second unknown-key cooldown is
  the bound.
- **Missing issuer as a development bypass.** F06. Replaced by fail-closed
  startup for API binaries regardless of role; the branch plan
  `5d1a0e96-6ece-4edc-9eff-18ae42ae6175` separated worker configuration so
  node and ingester start without browser settings.

## Consequences

- The backend depends on the broker's revocation-check route; during an
  overlapping rollout an old broker makes authenticated requests fail closed.
- Revocation reaches a live socket within eight seconds. Five-minute tokens
  mean the browser refreshes 60 seconds before expiry and shares one refresh
  across concurrent requests.
- A 60-minute Cognito JWKS outage makes authentication unavailable (503)
  rather than open; the user sees a retrying state, not the login screen.
- Revocation is Sulion-scoped; it does not perform a shared-pool Cognito
  global sign-out, and permanent repository secret grants survive it.
- The Ableton integration's endpoints are retired; its consumer cleanup is
  coordinated outside this repository.

## References

- Browser authentication and sign-out:
  [`../architecture.md`](../architecture.md#browser-authentication-and-sign-out)
- Audit findings F01, F03, F06, F07:
  [`../security/external-audit-2026-09-28.md`](../security/external-audit-2026-09-28.md)
- Remediation phases and release order: folded into
  [ADR 0027](0027-remove-device-access-and-allowlist-the-public-edge.md);
  rollback rules in [`../deploy.md`](../deploy.md), "Security release
  configuration"
- Commits: e5e8ab4 (Cognito), 3d40436 (device removal, revocation cutoff,
  fail-closed configuration), b2168f8 (recovery and scoped sign-out),
  521c20b and e180edb (503 during outages, key cache)
- Published plans: `f18004cd-fb95-4d3b-9136-913473b7686f`,
  `5d1a0e96-6ece-4edc-9eff-18ae42ae6175`,
  `a7930be4-6db5-4208-89d2-81e494021595`
