# External security remediation release

Release candidate for plan `f18004cd-fb95-4d3b-9136-913473b7686f`.
Publication was authorized on 2026-09-28. Publication and deployed evidence are
tracked in the published plan. Phase 7 remains excluded; this checklist contains
only the retained security gates.

## Review and release order

1. Review Sulion's diff together with the three Ahara Infra changes (WAF credential
   protection and obsolete pairing-rule removal, Cognito refresh trigger, strongly
   consistent entitlement reads). Ableton documentation marks the integration
   retired; its former client code is historical and must not be deployed.
2. After publication authorization, use the existing CI/CD pipeline for Terraform
   planning/application and deployment. It already holds the required AWS identity;
   terminal AWS access is not a prerequisite. Review its Terraform output for
   resource moves against deployed state. Sulion reuses only its existing
   listener priorities 173–177 and releases 178. The public edge forwards an
   allowlist; every other path reaches the shared listener's 404 default.
3. The shared workflow applies Sulion's Terraform before its TrueNAS image
   deployment, closing obsolete device and machine paths before backend/database
   retirement. The Cognito client move preserves
   its existing resource identity and changes token lifetimes; it must not
   recreate the client. Listener moves preserve five existing rule resources and
   remove the former `/*` catch-all.
   The edge module stays pinned at `7891b157df1cb83167eef2c9dfd8a25a6013cb75`;
   Sulion owns its claim conditions and path rules directly so this release
   does not require publishing a shared-module change.
4. Release broker migration 0005 and the backend/node/frontend changes together
   through the normal pipeline; the services run their own migrations at startup.
   Authenticated acceptance must wait for the new broker. Backend migration 0097 drops the unused device
   tables; never run ad hoc deletion against the deployed database. Inspect
   table dependencies and row counts without selecting hashes before release.
   The backend now needs the broker's revocation-check endpoint; an old broker
   makes authenticated requests fail closed during an overlapping rollout.
   Shells remain owned by devenv; no new orchestration mechanism is required.
5. The frontend derives its allowed S3 origin from the existing deployment's
   `SULION_UPLOAD_BUCKET` and backend region. `secret-paths.yml` already supplies
   the Terraform-managed bucket through CI/CD. The Cognito API origin comes from
   the existing pool ID. No independently configured origin is required.
6. After device-route closure, apply Ahara Infra's removal of the pairing-only
   WAF rule. Retain all shared rate limits and managed rules. Deploy its existing
   entitlement Lambda update and attach it to pre-token generation as well as
   pre-authentication. Its existing seeded-admin bypass remains unchanged.

Rollback must retain device and public-machine denials, the retired token tables,
and broker revocation storage/checks. Do not roll back to code that consults
device tokens or treats missing authentication configuration as a bypass.
Restore compatible binaries forward if necessary; do not reverse these migrations.

## Browser authority contract

- Cognito issues new access/ID tokens for five minutes. Sulion accepts access
  tokens only, validates signature/issuer/client and requires `exp`, `iat`,
  `auth_time` and `sub`. Tokens minted before the lifetime change retain their
  original expiry; use a controlled sign-out/re-login during acceptance.
- The broker database stores a monotonic per-principal cutoff in
  `secret_broker.browser_revocations`. Authentication times at or before the
  cutoff are rejected. Refreshing an old login cannot evade the cutoff because
  Cognito retains its original `auth_time`; a new interactive login is required.
- Sign-out revokes all prior Sulion logins for that principal before clearing
  local credentials. Failure is shown and can be retried; an already-revoked
  token acknowledges sign-out without advancing the cutoff for newer logins.
  Other applications'
  Cognito sessions and permanent repository secret grants are unaffected.
- REST and ticket issuance check revocation on each request. Tickets also carry
  their principal and access expiry, are single-use and expire after 30 seconds.
  Attach rechecks authority. Open sockets terminate at access expiry, or within
  eight seconds of revocation (five-second interval plus three-second deadline).
  A failed authority check closes the attachment. It never kills the PTY.
- Emergency revocation is an authenticated internal broker operation:
  `POST /v1/auth/revoke` with JSON `{"sub":"<Cognito subject>"}` and the broker
  registration credential in `Authorization: Bearer`. It is reachable through
  the pinned LAN gateway as `/broker/v1/auth/revoke`, never the public proxy.
  Use `with-cred --` in this environment, or an ordinary injected environment
  variable elsewhere; do not put the credential in a URL or shell history.
  Do not exercise this against the operator's production session during tests.

## Local evidence and remaining gates

Passed before publication: device-route/migration regression; 34 REST integration
tests including multipart through the node and symlink escapes; foreground S3
installation regression; 304 Rust unit tests across the full run and final auth
rerun, plus both structural checks; signed JWT/cooldown/outage and REST
revocation tests; 395 frontend tests;
two new sign-out tests; frontend typecheck, focused lint and production build;
strict Rust Clippy; Compose validation; nginx template rendering and `nginx -t`; static Terraform
validation for Sulion and Ahara Infra; five entitlement Lambda unit tests.
All 74 focused integration tests passed across device retirement, REST/uploads,
node protocol, retrieval, broker grants/revocation and WebSockets. The final
broker/socket rerun confirms durable sign-out, rejection of revoked management
requests, socket expiry/revocation and PTY survival. The socket fixture initially
intercepted PTY registration and then exposed a moved-listener compile error;
both fixture defects were corrected before the passing rerun. No live deployment
evidence is claimed by these results.

Frontend lint passed with five existing warnings. A new sign-out test initially
used an unsupported assertion matcher; its corrected two-test run passed.

Eleven Chromium tests passed against the built frontend nginx image: timeline
and source navigation, library actions, terminal rendering/reconnect/exit,
multipart uploads, signed secret redemption, security headers on HTML/API/error
responses, retired/internal route denial, blocked inline scripts and framing,
exact-origin S3 PUT permission, Markdown/math and SVG blob previews. The S3
response in the CSP test is a fixture; real IAM/CORS and Cognito MFA remain
deployed acceptance checks. Compose validation, origin derivation (including
disabled uploads and differing regions), Terraform validation, TypeScript,
focused ESLint and strict Rust Clippy passed after the deployment corrections.

The real-stack run exposed and corrected two implementation defects: shared
configuration incorrectly required browser-auth settings for node/ingester
binaries, and the frontend Docker context excluded the security-header file.
The API configuration remains fail-closed, including when given a worker role.
The nginx test harness now remains alive until teardown and Playwright requests
SIGTERM so its temporary containers can be removed.
The four security cases passed again with this teardown configuration; no
containers, network, volumes, frontend image or harness process from that run
remained afterward.

Remaining release checks:

- Publication is authorized for this release. Terraform and deployment run
  through the existing CI/CD pipeline, with its AWS identity. The earlier
  terminal AWS attempt in the audit was denied with exit 66:
  `credential-helper: broker denied access for aws (403 Forbidden): {"error":"no AWS credential is enabled for this terminal"}`.
  Ahara Infra's `terraform init -backend=false` also hit its existing S3 backend
  and failed with `No valid credential sources found`, `no EC2 IMDS role found`.
  Neither credential path was retried or replaced. These terminal limitations
  do not block CI/CD deployment. Credential-free static `terraform validate`
  succeeded independently.
- After release, record exact revisions and test rejected public routes with and
  without valid credentials from an independent external vantage point. Confirm
  LAN pinned node/broker/retrieval continuity, Cognito MFA, entitlement removal
  across refresh, sign-out/revocation, and a disposable real S3 upload. Inspect
  actual ALB/WAF/SG/Cognito/S3/IAM and log redaction using pipeline evidence.

The implementation uses the documented [ALB additional-claim conditions](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/lb_listener_rule),
[Cognito refresh trigger](https://docs.aws.amazon.com/cognito/latest/developerguide/user-pool-lambda-pre-token-generation.html),
and [WAF data protection](https://docs.aws.amazon.com/waf/latest/developerguide/logging-management.html).
WAF log redaction alone does not protect sampled requests, so credential fields
are protected at the web ACL as well; sampling and enforcement remain enabled.
