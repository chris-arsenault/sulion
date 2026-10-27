# ADR 0027: Device-Token Access Is Deleted, Machine Routes Are LAN-Only, and the Public Edge Forwards an Allowlist

## Status

Accepted, 2026-09-28; the public-edge allowlist amended 2026-09-30.

## Context

On 2026-06-14 commit `6bdb4d4` added OAuth-style device-authorization
pairing (`/api/devices/pair`, `/pair/token`, `/pair/approve`, a browser
`/pair` page) and device-token-authenticated ingest and raw-file routes,
built for an Ableton "Send to Sulion" extension. The tokens never expired,
had no repository or operation scope, and the listener carried ALB
exceptions so the pairing routes could be reached without a Cognito JWT.

An external access audit on 2026-09-28
(`docs/security/external-audit-2026-09-28.md`) found no anonymous shell or
repository-read bypass but eleven weaknesses, F01–F11. The ones that drove
this decision:

- F01: a stolen device token read and overwrote files across repositories
  with no MFA exchange, and no revocation UI existed.
- F05: anonymous pairing requests created database rows that nothing
  cleaned up.
- F04: retrieval, broker redemption, and PTY-credential registration were
  proxied to the Internet and accepted credentials in query strings, while
  their documented callers all used the pinned LAN listener on `:30081`.
- F02, F03, F06, F07, F08: symlink escapes in the legacy upload writer,
  no server-side browser revocation, a development auth bypass reachable by
  omitting an environment variable, unbounded JWKS refresh, and no browser
  security headers.

The user stated the device feature had never been used and directed
complete removal, with no compatibility window and no replacement token
service. The user also removed the audit's operational phase (resource
budgets, overload tests, readiness, monitoring, backups, recovery targets,
and the archive bucket's TLS policy) as disproportionate for a single-user
system; F09–F11 are recorded observations with no work items.

## Decision

Device access is deleted, not hardened. `device_routes.rs`, the device
ingest and raw handlers, the `/pair` page, and their tests are removed;
negative tests prove the retired routes are unavailable even with an old
token. Migration 0097 retires `device_pairings` and `device_tokens`
forward; historical migration 0052 is untouched, no ad hoc deletion runs
against the deployed database, and rollback must not re-enable the routes.
Listener priority 173's device exceptions and `ahara-infra`'s pairing-only
WAF rate rule go with them. The Ableton integration is recorded as retired
in `docs/ableton-file-contract.md`.

Machine services are LAN-only. `/retrieval`, `/broker/v1/use`, and
`/broker/v1/pty-credentials` are denied at the frontend proxy and at the
edge; query-string credentials are removed from broker and retrieval
authentication; credential headers and ticket subprotocols are redacted in
edge logs; public health no longer discloses internal service URLs. Node
registration, wrapper redemption, and retrieval stay on the pinned LAN
gateway. Browser secret and grant management stays public but
authenticated.

The public edge forwards an allowlist. The 2026-09-28 release kept the
unauthenticated `/*` forward and added per-route denials, which needed a
new listener rule at priority 172. That priority collided with another
project's rule on the shared listener. On 2026-09-30 commit `d4d615a`
replaced the catch-all: the ALB forwards only the SPA shell, build assets,
WebSocket, and health; denials remain for machine routes under `/api` and
`/broker`; every other path reaches the shared listener's 404 default.
Sulion reuses only its existing priorities 173–177 and releases 178. The
edge module stays pinned at `7891b15`; Sulion owns its claim conditions and
path rules directly.

Browser authority is bounded. Cognito tokens last five minutes. The broker
stores a monotonic per-principal cutoff compared against `auth_time`
(broker migration 0005), so refreshing an old login cannot restore revoked
access. REST, broker management, and ticket issuance check it per request;
attached sockets recheck every five seconds and terminate at access expiry
or within eight seconds of revocation, without killing the devenv-owned
PTY. Sign-out revokes all prior logins for the principal; emergency
revocation is a LAN-only broker operation. The identity integration's
entitlement check also runs on refresh, not only at sign-in.

Authentication fails closed: production roles and the broker refuse to
start without a nonblank HTTPS issuer and client id; any bypass is an
explicit test or development choice. JWKS refresh is coalesced behind one
in-flight fetch with finite deadlines and an unknown-key cooldown. The
frontend serves CSP with `frame-ancestors 'none'`, nosniff, a restrictive
referrer policy, and HTTPS-only HSTS; the CSP's S3 and Cognito origins are
derived from `SULION_UPLOAD_BUCKET` and the user pool id rather than
configured separately. Legacy multipart uploads install through the same
descriptor-relative writer the S3 path uses (ADR 0026).

Release order closes public paths before retiring tables: Terraform before
the TrueNAS image, broker migration together with backend, node, and
frontend. Rollback retains the denials, the retired tables, and the broker
revocation checks.

## Alternatives considered

- **Harden device tokens with a compatibility window.** Expiry, scopes,
  revocation UI, and pairing cleanup. Rejected by the user: the feature had
  never been used, so there was nothing to keep compatible. The plan
  records "delete rather than harden" for F05 and "do not invest in a
  replacement device credential design" for F01. A hardened token system
  would still have been a second, MFA-free credential beside Cognito.
- **Per-route edge denials on the `/*` catch-all.** The 2026-09-28 shape.
  Rejected two days later when, as the body of `d4d615a` records, the
  added priority-172 rule collided with another project on the shared
  listener. A catch-all also meant every new backend route was public
  unless someone remembered a denial; an allowlist makes a new route
  private by default.
- **Keep machine routes public with query-token authentication.** The
  prior state, documented under F04. Rejected because the callers were all
  on the LAN, a copied static token was usable from the Internet, and
  query credentials entered ALB and nginx logs that the WAF did not
  redact. Closing the paths removed the exposure without changing the LAN
  protocol.
- **Operational hardening (the audit's phase 7).** Out of scope at the
  user's direction, recorded in the plan's scope correction and finding
  table. F09 (request admission and resource ceilings), F10 (single
  components and readiness), and F11 (archive bucket TLS policy) are
  observations without implementation or completion gates, and the plan
  states they do not transfer to a later phase. This ADR records that
  disposition; it is not a deferred queue.

## Consequences

- Sulion has one browser credential path: Cognito with MFA, five-minute
  tokens, and server-side revocation. There is no long-lived machine
  credential for external tools, and the Ableton client is retired.
- The Internet reaches the SPA, its assets, the browser WebSocket, and
  health. Every other route, including ones added later, is unreachable
  from outside until a listener rule forwards it.
- An old broker makes authenticated requests fail closed during an
  overlapping rollout, since the backend now needs the revocation endpoint.
- Deployments that omit the issuer or client id do not start; browser
  attachments drop within eight seconds of revocation and PTYs survive.
- The plan was closed on 2026-10-01 after public unauthenticated probes
  returned 404 for pairing, broker use, PTY registration, and retrieval,
  and HTML carried the security headers. Authenticated deployed acceptance
  for F01–F08 was not completed; the user accepted closure with that gate
  skipped.

## References

- Audit and findings:
  [`../security/external-audit-2026-09-28.md`](../security/external-audit-2026-09-28.md)
- Browser authority contract as shipped:
  [`../architecture.md`](../architecture.md), "Browser authentication and
  sign-out"; rollback rule in [`../deploy.md`](../deploy.md), "Security
  release configuration"
- Published Sulion plan `f18004cd-fb95-4d3b-9136-913473b7686f`; its phases,
  scope correction, finding coverage, and release order are folded into
  this ADR
- Retired surface: [`../architecture.md`](../architecture.md), "Retired
  device access"; [`../ableton-file-contract.md`](../ableton-file-contract.md)
- Upload writer shared with the S3 path:
  [`0026-foreground-s3-upload-staging.md`](0026-foreground-s3-upload-staging.md)
- Commits: `6bdb4d4` — Add Ableton device pairing + MIDI ingest (the
  surface removed); `3d40436` — Remove device access and harden external
  authentication; `d4d615a` — Replace public ALB catch-all with a path
  allowlist
