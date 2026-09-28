# External access and availability audit — 2026-09-28

Sulion is reachable through its public hostname. The anonymous read and
WebSocket checks performed here were rejected at the expected boundaries.
There is no demonstrated anonymous shell or repository-read bypass. There are
material weaknesses in the unused device-token surface, file-write confinement,
credential revocation, and the extent of public machine-service exposure.

The user's direction is to remove device-token access completely. The
[remediation plan](../plans/external-security-remediation.md) maps F01–F08 to
security work and verification. The user subsequently removed operational
phase 7: F09–F11 remain recorded observations outside remediation scope, with no
implementation or completion gates. Phase 3's internal-service LAN restriction
is agreed. No application or deployed configuration was changed during this audit.

## Scope and evidence limits

- Reviewed Sulion at `1f5e899334dada7e1109f6a70f0ab9495ee598ee`, plus working
  changes belonging to the active permanent-repository-secret-grants plan.
  Those changes were not modified or treated as deployed.
- Read related local infrastructure: ahara-infra
  `2bf2649c01106420dd61f66c041e807eee63438c`, ahara-vpn
  `7a54e02f0f625ddf64fdf2c6eebf8bc9a7822096`, and ahara-tf-patterns
  `7891b157df1cb83167eef2c9dfd8a25a6013cb75`.
- HTTP checks used the public hostname from this managed terminal, with normal
  TLS verification, around 05:59–06:02 UTC. This proves the public DNS/ALB path
  works from this origin; it is not an independent off-site network scan.
- No real credentials, device pairings, uploads, sessions or grants were created
  for testing. No production load test, port scan or fault injection was run.
- AWS inspection stopped on the intended credential boundary. The attempted
  `aws elbv2 describe-load-balancers` command exited **66**:

  ```text
  credential-helper: broker denied access for aws (403 Forbidden): {"error":"no AWS credential is enabled for this terminal"}
  ```

  Live listener rules, WAF configuration/logs, security groups, Cognito settings,
  IAM, S3 policies, image revisions and recovery state therefore remain
  unverified. No alternative credential source was sought.
- This is a focused code/configuration audit, not an exhaustive dependency-CVE
  assessment, penetration test, or certification of the complete home network.

## Reachability and controls

The checked-in public route is:

```text
Internet HTTPS → shared ALB + WAF → EC2 nginx → WireGuard
  → TrueNAS frontend :30080 → API / broker / retrieval

Development node → pinned TLS on TrueNAS :30081
  → /ws/nodes and broker/retrieval gateway
```

| Check | Observed result |
|---|---|
| HTTP `/` | ALB 301 redirect to HTTPS |
| HTTPS `/` | 200; no CSP, framing, HSTS, nosniff or referrer-policy response headers |
| `/health` | 200; database OK and development node connected |
| `/api/app-state`, without credentials | ALB 401 |
| `/broker/v1/secrets`, without credentials | ALB 401 |
| `/ws/nodes` | nginx 404 |
| WebSocket upgrade to a fixture session UUID, without a ticket | Application 401 |
| `/api/repos/sulion/raw?path=README.md`, without credentials | Application 401; no file returned |
| `/retrieval/v1/index/status`, without credentials | Application 401 |
| `/retrieval/health`, without credentials | 200; discloses internal embedding URL, model and index capabilities |

Controls worth retaining:

- `infrastructure/terraform/api.tf` puts browser API and broker management
  behind ALB JWT validation. `backend/src/auth.rs` independently validates
  RS256 signatures, issuer, expiration through the JWT library, and the
  application client claim. The shared ALB module currently specifies issuer
  and JWKS, without Sulion-specific client/token-use claim conditions; the
  backend supplies that additional check.
- Shared Cognito Terraform requires software-token MFA. Its pre-authentication
  trigger checks per-user application access, with a seeded-admin exception.
  These are source observations, not a live MFA/account-membership audit.
- Browser WebSockets require random 256-bit, hashed, session-bound, single-use
  tickets with a 30-second issuance lifetime. Credentials stay out of their URLs.
- The frontend explicitly rejects `/ws/nodes`. The separate node listener uses
  a real-peer LAN check, a signed challenge, operator approval and pinned TLS.
  Untrusted forwarded-address headers do not satisfy its source check.
- The gateway's checked-in default-drop policy allows AWS traffic to frontend
  port 30080 but excludes node port 30081 and development ports 26000–26010.
  Development-port access is separately allowed from secure client networks.
  Reverse-proxy HTTP ingress in AWS is limited to the ALB security group.
- WAF retains IP reputation, managed common rules, oversized-body enforcement,
  a shared 2,000-request IP rate rule and a 100-request pairing-start IP rule.
  The configured windows default to five minutes. These are approximate rate
  controls, not strict application concurrency limits. See
  [AWS rate-rule settings](https://docs.aws.amazon.com/waf/latest/developerguide/waf-rule-statement-type-rate-based-high-level-settings.html).
- The foreground S3 upload flow binds subject, destination, size and checksum;
  PUT grants expire after 900 seconds and require conditional creation. Downloads
  reject redirects, enforce host/key, size and checksum, use two transfer slots,
  and install through directory descriptors with atomic replacement.
- The broker has separate encrypted storage and a separate master-key mount.
  Redemption checks a PTY signature, timestamp, replay nonce and active grant.
  Browser reads conceal stored secret values.

## Findings

Severity reflects impact and prerequisites, not a claim that exploitation has
occurred. Source-confirmed findings have not been exercised against production.

### F01 — High: permanent device credentials authorize broad repository access

`backend/src/api/device_routes.rs:301` accepts any matching unrevoked token;
there is no expiry, repository restriction or operation scope. The principal
does not constrain `post_repo_ingest` or `get_repo_raw` in
`backend/src/api/repo_routes.rs:463`. The schema explicitly makes these tokens
non-expiring. No production token revocation API/UI was found in the reviewed
surface, although the database has a `revoked_at` field.

A stolen, previously approved token can read files and overwrite content across
repositories without another Cognito/MFA exchange. Writing source or scripts
can lead to execution when the operator or agent later uses them. This requires
a valid device credential; anonymous token minting is not possible without
approval. Live token counts were not inspected, and the user states the feature
has never been used.

**Disposition:** remove the entire feature, including state, UI and edge
exceptions. Do not invest in a replacement device credential design. Phase 1.

### F02 — High: legacy uploads escape the repository through symlinks

`backend/src/workspace.rs:34` verifies containment only if the complete target
exists. `write_file` at line 239 checks the immediate parent only if it exists,
otherwise calls `create_dir_all`, then writes through the path.

Two cases were reproduced against the actual Rust function in disposable
directories:

1. `repo/link` points outside the repo; `link/new-directory/probe.txt` has a
   missing immediate parent. The write creates the directory and file outside.
2. `repo/dangling` points to an outside file that does not yet exist. Existence
   checks follow the symlink and report false; the final write follows it.

An escaping symlink must already exist or be introduced through repository
content or another writer. A check-then-write race also remains in this design;
the two deterministic cases above do not require a race. The affected writer is
used by device ingest and browser multipart repository/workspace uploads through
`backend/src/node_runtime/requests.rs`. Removing device access alone does not
remove the bug. The S3 installer uses a different, safer implementation.

**Disposition:** reuse `backend/src/uploads/install.rs` for remaining upload
writes; reject symlink directory traversal and replace final names atomically
without following them. Phase 2.

### F03 — High: browser authority has no complete server-side revocation path

`backend/src/auth.rs` validates JWTs offline without a revocation lookup.
`backend/src/api/ws.rs:154` consumes a ticket and then retains neither the
principal nor the originating JWT expiry on the attachment. The socket loop
has no authorization expiry/recheck. A client that already attached can keep
using that connection after its original JWT expires until a separate close,
disconnect or runtime event ends it. Ordinary frontend sign-out is local SDK
sign-out, not a server-enforced revocation of a malicious client's connection.

The shared pre-authentication trigger gates new sign-ins. Cognito does not run
that trigger on session renewal, so removing an application grant alone does
not cover refresh. Offline signature/expiry checks also do not detect revoked
JWTs. These limitations are documented by
[Cognito pre-authentication](https://docs.aws.amazon.com/cognito/latest/developerguide/user-pool-lambda-pre-authentication.html)
and [JWT verification](https://docs.aws.amazon.com/cognito/latest/developerguide/amazon-cognito-user-pools-using-tokens-verifying-a-jwt.html).

This is a containment and offboarding defect after credential/session compromise,
not an unauthenticated login bypass. No live revocation experiment was performed.

**Disposition:** define one bounded revocation contract for REST, broker
management, ticket issuance and live attachments; preserve underlying PTYs when
disconnecting browsers. Cover refresh authorization in the identity integration.
Phase 5.

### F04 — Medium: machine services remain public and accept URL credentials

`frontend/nginx.conf` proxies all `/retrieval/` and `/broker/` paths.
`infrastructure/terraform/api.tf` exempts retrieval, broker redemption and PTY
credential registration from ALB JWT checks. They still have application auth:
the issue is exposure to the Internet when their documented callers use pinned
LAN TLS, not a missing-auth claim.

`backend/src/retrieval.rs:376` accepts the static service token from either
headers or `access_token` query parameters. Broker authentication also accepts
query credentials. A copied service token therefore remains usable remotely;
the retrieval token covers search, reindex and reset operations. Query tokens
can enter ALB/nginx request logs. WAF redacts Authorization, Cookie and X-API-Key,
but the reviewed configuration does not redact query tokens, the alternate
retrieval-token header or ticket subprotocol headers. No actual leakage was
verified. Public retrieval health additionally discloses the internal embedding
address; this disclosure alone is low severity.

**Disposition:** close these machine paths at the browser proxy and edge,
preserve the pinned LAN gateway and authenticated browser management, remove URL
credentials, and align log redaction. Phase 3.

### F05 — Medium: anonymous pairing creates persistent database work

`start_pairing` inserts a row for each accepted request. The 900-second TTL limits
credential validity, not retention; no cleanup of `device_pairings` was found in
the application or migrations. `poll_token` opens a transaction and row lock;
its advertised two-second polling interval is not enforced server-side.
The client label has no small field-specific bound, although request-body and
WAF limits constrain it. Per-IP WAF limits mitigate one origin but do not bound
aggregate distributed writes or guarantee cleanup.

No flooding was performed and current table size is unknown.
**Disposition:** eliminated by complete removal in phase 1; do not add a new
pairing cleanup/rate-limit subsystem for a retired feature.

### F06 — Medium, configuration-dependent: missing issuer disables backend auth

`backend/src/config.rs:140` returns `None` if `SULION_AUTH_ISSUER_URL` is absent;
`require_http_auth` then inserts a synthetic dev principal and allows requests.
There is no production-role guard. This is not evidence that deployed auth is
disabled: Compose explicitly passes the variables, and an empty issuer is not
the same as an absent issuer in this code. ALB rejection was observed live.

An omitted variable in a changed deployment or direct listener configuration
can silently turn intended authentication off. ALB issuer-only validation is
also a weaker backstop than matching Sulion's client and token use at both layers.

**Disposition:** explicit, isolated test/dev bypass; production startup requires
valid issuer and client. Verify client/token-use checks at both layers. Phase 4.

### F07 — Medium: JWKS refresh is unbounded and amplifies outages

`backend/src/auth.rs:69` refreshes on every missing key or expired 15-minute cache.
It has no shared in-flight refresh, negative-key cooldown or explicit connect/
request timeout. The selected reqwest client defaults do not supply a total
request timeout. Concurrent legitimate reconnects can duplicate refreshes;
an identity-service outage makes all expired-cache authentication depend on it.

The public ALB rejects invalid JWTs before ordinary protected handlers, so this
is not a demonstrated Internet-to-backend unknown-key flood. It matters during
valid reconnect bursts, direct LAN calls and gateway/configuration mistakes.

**Disposition:** bounded/coalesced refresh, controlled unknown-key retry, early
algorithm rejection, and tests for key rotation and outage behavior. Phase 4.

### F08 — Medium: browser security response policy is absent

The live HTML response lacks CSP, frame-ancestors/X-Frame-Options, HSTS,
X-Content-Type-Options and Referrer-Policy. No equivalent policy was found in the
reviewed frontend/proxy source. The Cognito SDK uses its default browser storage,
so a successful same-origin script injection would have serious consequences.
No script injection exploit was demonstrated. File rendering already keeps SVG
inside images and displays HTML as source; those protections should remain.

**Disposition:** enforced, tested response policies compatible with the actual
application; HTTPS-only HSTS and no unreviewed includeSubDomains policy. Phase 6.

### F09 — Medium, hardening gap: application resource admission is incomplete

The browser API router has no global request-concurrency or deadline layer.
Terminal attachments have no per-principal/session admission limit or explicit
application message/frame cap. Frontend nginx uses 3,600-second upstream read
timeouts. Compose defines no control-plane service memory/CPU/PID ceilings.

There are useful bounds: a 16-connection backend DB pool with a 10-second acquire
timeout, nginx connection limits, HTTP body limits, bounded internal channels,
node request deadlines and the two-slot S3 installer. Therefore this is not a
claim of universally unbounded processing or a proven outage. WAF limits HTTP
request rates; they do not cap work inside an established terminal connection.

**Disposition:** outside remediation scope at the user's explicit direction.
The proposed resource-limit and operational-testing project was removed with
phase 7. This observation creates no implementation or completion gate.

### F10 — Medium, operational risk: remote availability depends on single components

The configured ALB has two subnet placements but one reverse-proxy target.
Remote operation then depends on the tunnel/gateway, home connectivity, TrueNAS
frontend/control/database and dedicated development node. There is no automatic
replacement for these single components in the reviewed topology.

`/health` returns 200 when Postgres is reachable even if the node is unavailable;
it exposes that state in JSON. This is appropriate for retained-history access,
but a status-only monitor cannot establish terminal readiness. Broker health is
liveness-only. Shared ALB health checks do not prove Sulion login, uploads or PTY
availability. Existing external alerts and recovery timing were not verified.

The monthly durable dump plus verified transcript archive is useful recovery
evidence in code, not proof of a current full backup. Broker database/key,
node repositories, recent transcripts and identity configuration require their
own recovery coverage. No restore-time or data-loss guarantee is established here.

**Disposition:** outside remediation scope at the user's explicit direction.
Keep the single-user topology. No readiness redesign, monitoring/alert expansion,
backup change, recovery target or restore exercise is required by this plan.

### F11 — Low: archive bucket lacks enforced TLS-only access

`infrastructure/terraform/archive.tf` defines private access, encryption,
versioning and retention, but no deny on insecure transport. The current SDK
uses HTTPS. Unlike the upload bucket, a future authorized client is not prevented
by bucket policy from using HTTP. No cleartext transfer was observed.

The current ahara-infra bucket-policy grant is scoped to the upload bucket;
adding archive enforcement needs a reviewed, narrowly scoped provisioning grant.
**Disposition:** outside this plan with removed phase 7. No archive policy or IAM
change is authorized by the plan, and no related work transfers to phase 8.

## Availability constraints and accepted trust boundaries

- Sulion's WAF path still blocks oversized bodies and signature-matching text.
  The managed size rule's threshold is 8 KiB; S3 solves file-byte transport,
  not arbitrarily large JSON prompts. This is an existing product constraint,
  not a separate remediation project or a reason to disable LFI/XSS rules.
  See [AWS managed core rules](https://docs.aws.amazon.com/waf/latest/developerguide/aws-managed-rule-groups-baseline.html).
- WAF is not prompt-injection protection. An authorized user can intentionally
  execute shell commands, including commands entered over a terminal socket.
- PTYs share a UID, and dedicated-mode PTYs have direct Docker access. Treat
  a compromised authenticated operator/agent as potentially compromising the
  development host and secrets already granted there. The broker's separate
  master key does not make unlocked values isolated between same-UID PTYs.
- Permanent repository secret grants are independently authorized work in
  progress. This audit does not revoke or redesign that requirement. Machine
  registration tokens, device tokens and repository grants are different things.
- No public routing of node-control, development ports or administrative SSH is
  introduced by the remediation plan.

## Verification performed

- 14 node LAN/source-admission unit tests passed.
- 2 WebSocket ticket tests passed, covering expiry, single use, session binding
  and subprotocol extraction.
- 3 S3 upload tests passed, covering signed size/conditional creation, subject/
  destination binding and symlink-safe atomic installation.
- A separate Cargo reproducer depended on the actual local Sulion library and
  confirmed both F02 cases. Its source is retained locally at
  `/tmp/sulion-security-audit-2026-09-28/reproducer/`; its disposable data was
  deleted automatically. It did not start any service or access production.
- The unit build used the working tree while the independent broker-grant task
  was active. These results validate the listed mechanisms, not a frozen release.

Authenticated live behavior, deployed rule parity, isolated full-stack security
regressions, TLS protocol enumeration, independent external reachability,
backup restoration and dependency vulnerabilities were not established by this
audit. These limitations are not automatic follow-up work. Plan completion is
limited to the retained security phases and their stated verification gates;
F09–F11 are excluded and do not block completion.
# Remediation status

The audit below records the pre-change evidence. Local remediation now removes
device access (F01/F05), confines legacy uploads (F02), closes public machine
paths and removes URL credentials (F04), fails closed on missing auth config
(F06), bounds JWKS refreshes (F07), adds durable browser revocation/expiry (F03)
and enforces browser response policy (F08). See the
[release record](external-remediation-release.md) for test evidence, release order
and residual limits. These changes are unpublished and are not deployed proof.
F09–F11 remain excluded. Live/browser acceptance still gates plan completion.
