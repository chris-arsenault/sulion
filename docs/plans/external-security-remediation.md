# External security remediation

Published plan: `f18004cd-fb95-4d3b-9136-913473b7686f`.
Status: phases 1–6 implemented and locally verified. Eleven Chromium checks
passed against the production nginx image. The four security cases passed again
with graceful teardown; their containers, network, volumes and image were removed.
Phase 8 local checks passed; publication is authorized and underway.
Pipeline and deployed acceptance remain pending in the published plan.
Original phase 7 is removed from scope
and marked skipped in the published history; phase numbers remain stable.
Evidence: [2026-09-28 audit](../security/external-audit-2026-09-28.md).
Execution evidence and release order: [release record](../security/external-remediation-release.md).

Outcome: remove unused device access and reduce Sulion's external attack surface
while preserving browser operation, LAN-only nodes, foreground uploads and the
current single-operator deployment.

The user explicitly directed complete device-token removal because it has never
been used. There is no compatibility window or replacement token service.
The user has authorized execution of the retained phases, including temporary
test servers with cleanup. Publication and long-running development servers
require explicit authorization. AWS changes run through the existing CI/CD
pipeline; terminal AWS access is neither expected nor a deployment prerequisite.

Scope correction: the user requested security work only and explicitly removed
phase 7 as disproportionate operational engineering for a single-user system.
Its resource budgets, overload tests, readiness redesign, monitoring/alerts,
backup changes, restore exercises and recovery targets are not work items or
completion gates. The archive TLS policy bundled into that phase is also outside
this plan, not moved to another phase. Phase 3's direction is explicitly agreed.

## Principles and dependencies

- Keep Cognito MFA, backend claim validation, WAF protection, pinned node TLS,
  secret-broker separation and existing source/ingestion ownership.
- Reuse existing safe implementations. In particular, share descriptor-relative
  upload installation instead of building another filesystem abstraction.
- Keep the shared single-operator model. Do not introduce per-user workspaces,
  tenant ACLs, a general permissions service or automatic HA infrastructure.
- Present any proposed scope addition to the user before encoding it as plan
  work. Audit observations do not automatically authorize implementation.
- Verify caller contracts before closing machine paths. Browser secret setup and
  grant changes remain public but authenticated; node registration and wrapper
  redemption remain available on the pinned LAN listener.
- Coordinate overlapping files with permanent-repository-secret-grants plan
  `7b329adc-5cfe-4d57-8340-3314f53dc839` which has now landed.
  Preserve its non-expiring repository grants and signed redemption semantics.
- Preserve upload plan `0b162a4c-bfdd-4c08-824f-29c223326c49`: no ledger, queue,
  worker, recovery UI, automatic retry or public multipart fallback. Its live
  upload verification is still outstanding in the published record.
- Current branches only. Publication needs explicit authorization. Follow the
  repository integration harness and credential boundaries. At phase start read
  `sulion plan current` and the actual affected sources; branch multi-step blockers.

## Finding coverage

| Finding | Resolution phase |
|---|---|
| F01 Permanent, broad device credentials | 1: complete deletion |
| F02 Symlink escapes in legacy uploads | 2: safe shared writer |
| F03 JWT/refresh/attached-socket revocation gaps | 5: bounded browser authority |
| F04 Public machine routes and URL credentials | 3: LAN boundary and log hygiene |
| F05 Persistent anonymous pairing work | 1: delete rather than harden |
| F06 Missing-issuer development bypass | 4: explicit production configuration |
| F07 JWKS refresh fanout/timeouts | 4: bounded refresh |
| F08 Missing browser security headers | 6: enforced response policy |
| F09 Incomplete application resource limits | Excluded at user direction; no implementation or validation gate |
| F10 Readiness, single components, recovery proof | Excluded at user direction; retain the single-user topology |
| F11 Archive insecure-transport policy gap | Outside this plan with removed phase 7; no implicit follow-up |
| Unverified deployment parity and upload behavior | 8: deployed acceptance |
| WAF body/content restrictions | Preserve existing protection; no separate WAF behavior/error project |

## Phase 1 — Remove device access completely

Delete `backend/src/api/device_routes.rs`, its router wiring, device ingest/raw
handlers and unused types. Keep Cognito-authenticated `/file/raw` and browser
multipart/S3 routes. Remove `/pair`, `PairPage`, its API helper and retired
pairing tests. Replace happy-path device tests with negative coverage proving
all retired routes are unavailable even when presented with an old token.

Remove listener priority 173's device exceptions in `infrastructure/terraform/api.tf`.
In ahara-infra remove only `SulionPairingStartRateLimit` after route closure;
retain the shared rate limit and managed rules. Deleting an ALB exception alone
is insufficient because the catch-all still forwards and the backend route
could remain reachable with a different edge-valid credential.

Retire `device_pairings` and `device_tokens` with a new forward migration; never
edit historical migration 0052 or reuse another active task's migration number.
Code must stop consulting tokens immediately. Inventory dependencies and counts
without printing token hashes. Any deployed destructive migration occurs only
with the eventual authorized release; do not run ad hoc database deletion now.
Rollbacks must not re-enable the old routes or restore usable token state.

Remove device-only public-URL configuration if it has no remaining caller.
Update architecture/deploy docs. The local ableton-extensions contract explicitly
depends on these endpoints; record the integration as retired and coordinate
its consumer cleanup rather than creating a compatibility shim or redesigning
Ableton functionality. Transcript ingestion and development-node pairing are
unrelated and must remain intact.

Acceptance: old routes produce a non-success response in the backend and through
the browser proxy with and without credentials; no token issuance/lookup remains;
fresh and upgraded databases migrate; normal file viewing and uploads still work.

## Phase 2 — Confine every upload to its repository

Route `workspace::write_file` callers through the existing directory-descriptor
installer. Preserve existing overwrite behavior and return values; do not follow
directory symlinks, dangling final symlinks or paths swapped during the operation.
Reuse size bounds and atomic replacement for repository and workspace uploads.

Acceptance: executable regressions for the two demonstrated escapes, existing
outside symlinks, traversal, final-name replacement and destination changes.
Assert outside fixtures remain untouched and successful uploads appear atomically.
Exercise the node protocol and HTTP multipart paths, not only helper functions.

## Phase 3 — Restrict machine services to LAN

Direction agreed by the user after clarification. These are internal retrieval,
secret-redemption and PTY-key-registration services, separate from device access.

Inventory callers in node bootstrap, wrappers, retrieval CLI, standalone and E2E.
Then deny public `/retrieval` and `/retrieval/*`, `/broker/v1/use`, and
`/broker/v1/pty-credentials` plus descendants. Use both frontend rejection and
explicit edge denial/removal that cannot fall through to the static catch-all.
Preserve broker browser-management routes and the encrypted LAN gateway.

Delete query-token authentication from broker/retrieval. Retain Authorization
headers and only genuinely required alternate headers; redact any retained
credential headers and ticket subprotocols in edge logs. Stop logging complete
gateway URLs containing credentials. Public health must not disclose internal
service URLs; detailed diagnostics belong behind the existing trusted boundary.

Acceptance: public requests to retired machine routes fail even with otherwise
valid credentials; signed LAN redemption/registration and retrieval still work;
query credentials are rejected and request logging cannot expose them. No port
30081 proxy registration or wider firewall rule is introduced.

## Phase 4 — Make authentication fail closed and bounded

Require nonblank valid issuer/client settings for production roles and broker
startup. Any test/local bypass must be an explicit test/development choice,
rejected by production configuration. Extend deployment validation accordingly.

Coalesce JWKS refresh behind one in-flight operation; apply finite connect/read
deadlines and a bounded refresh cooldown for unknown keys. Preserve signature,
issuer, expiry and client validation during outages. Do not allow unknown keys
or indefinitely extend stale-key acceptance. Check algorithm before network work.
Verify and add Sulion client/token-use conditions at the ALB where supported;
retain independent backend checks. Pin the consumed edge module revision when
changing its security contract, rather than depending on an unreviewed update.

Acceptance: signed fixture JWTs cover correct/wrong issuer and client, expiration,
missing claims, wrong algorithm, invalid signature and key rotation. Parallel
cache misses produce bounded network work; outage latency is bounded; missing
production configuration prevents serving rather than opening the API.

## Phase 5 — Bound and revoke browser authority

Implementation contract: the broker owns the durable per-principal cutoff.
All browser requests/tickets check it, and sockets recheck every five seconds
with a three-second timeout. Cutoffs compare Cognito `auth_time`, preventing
refresh of an old login from restoring revoked authority. Sign-out revokes all
prior Sulion sessions for the principal and leaves PTYs/grants alive. New tokens
last five minutes; pre-release tokens retain their original signed expiry.

Define one revocation contract before implementation. Recommended mechanism:
short explicit access-token lifetime, tickets carrying verified principal and
expiry, attachments ending at authorization expiry, and a durable per-principal
revoke-before timestamp checked by REST/broker/ticket validation and propagated
to live browser attachments. Reconnect may obtain a fresh ticket using a valid
current session. Disconnecting a browser must not kill its devenv-owned PTY.

The shared identity integration must re-evaluate application entitlement during
refresh, not only initial sign-in. Read its actual trigger/configuration and
coordinate that cross-repository change; do not infer it from Cognito labels.
Specify the maximum revocation propagation delay and test it. Normal sign-out
must revoke the browser authority it promises to revoke; emergency revocation
must cover existing sockets. Do not create a replacement device-token system.

Acceptance: expired/revoked credentials cannot mint tickets or manage secrets;
an attached malicious client loses authority within the stated bound; refresh
cannot restore removed application access. Preserve PTY survival and the active
repository-secret-grant contract. Test against controlled identities, never by
revoking the user's production session during implementation.

## Phase 6 — Harden browser responses

Add CSP with a narrow script/connect policy, `frame-ancestors 'none'`,
`object-src 'none'`, `base-uri 'none'`, framing protection, nosniff and a restrictive
referrer policy. Inventory actual Cognito, WebSocket, S3, font, blob and rendering
requirements before finalizing directives. Do not ship report-only as the final
control, add broad wildcard/unsafe script permissions, or send reports to a new
hosted service. Apply HSTS at the HTTPS boundary, without changing LAN HTTP or
asserting policy over unrelated subdomains.

Acceptance: response headers on HTML/API/error paths plus browser tests for login,
terminal, Markdown/math, highlighted source, SVG images and S3 uploads. Keep SVG
out of inline executable markup. Use a local framing fixture to prove denial.

## Phase 7 — Removed from scope

Skipped at the user's explicit direction. No work or acceptance criteria remain
in this phase, and none transfer to phase 8. The scope correction and finding
coverage above record the disposition without creating a deferred work queue.

## Phase 8 — Verify deployed controls and retire findings

Run focused unit checks, registered isolated backend integration tests, frontend
checks, Compose validation and Terraform validation for affected repositories.
Browser E2E uses temporary servers that are cleaned up after testing; no
additional permission is needed. A skip is reported, not converted into a pass.

Prepare exact reviewed diffs and release order before requesting publication.
Prefer closing public obsolete paths before removing shared rules and retiring
tables. Keep node/control independent release ordering valid. Validate necessary
infra/module prerequisites before consumer rollout. Rollback must preserve
device-route closure and not undo revocation or reopen machine routes.

After authorized publication, verify exact deployed revisions and repeat the
public rejection matrix from an independently external vantage point. Use the
existing CI/CD pipeline's AWS identity and deployment evidence for infrastructure
validation, including ALB/WAF/SG/Cognito/S3/IAM. Do not request standing AWS access
for this terminal or manually apply Terraform. Prove pinned
LAN node/broker/retrieval continuity, Cognito MFA, browser revocation and a real
foreground S3 upload with a disposable fixture. Never put real tokens in URLs.

Update each in-scope audit finding (F01–F08) with its change, test evidence,
deployment evidence and residual limitation. F09–F11 retain their excluded
disposition and do not block completion. Complete the published plan only after
the seven retained phases meet their gates or the user explicitly accepts a
documented residual risk. If a
prerequisite needs a separate multi-step repair, mark the phase blocked and use
`sulion plan branch`; do not silently drop its acceptance criteria.
