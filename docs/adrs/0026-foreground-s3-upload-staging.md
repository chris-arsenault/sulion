# ADR 0026: Remote Uploads Stage Through a Private Presigned S3 Bucket as Foreground Operations

## Status

Accepted, 2026-09-25.

## Context

Explorer uploads, pasted text files, and clipboard images reach the node
through multipart `POST` routes on the backend. On the LAN that path is
direct. On `https://sulion.services.ahara.io` every request crosses the
shared Ahara ALB and WAF, whose managed core rule group enforces an 8 KiB
body-size threshold and signature rules on request text. A multipart file
body of any useful size is blocked at the edge before Sulion sees it. The
WAF is shared with other Ahara services and its protections were to stay as
they are.

The first design for a remote path was a durable delivery subsystem: an
uploads database migration, a dispatcher, a retry and reconciliation state
machine, status APIs, a global browser store, and an uploads panel. On
2026-09-25 the user rejected it as more than the problem warranted. The
simplified replacement was authorised the same day and published after its
companion `ahara-infra` change.

## Decision

Remote uploads move file bytes through a private S3 staging bucket and keep
only small authenticated metadata on the application ingress:

1. The browser sends repository or workspace, directory, filename, byte
   count, and SHA-256 to `POST /api/uploads`.
2. Control validates the metadata, resolves the destination node, and
   returns a generated UUID and a constrained presigned S3 `PUT`.
3. The browser `PUT`s the raw file to S3 directly, with no Sulion cookies or
   credentials.
4. The browser sends the same metadata to `POST /api/uploads/:id/complete`.
   Control checks the object's size, checksum, and signed metadata binding,
   then asks the node to download and install it within the same request.
5. The node returns the existing installed `{path, size}` result, and the
   UI inserts the path or refreshes the file list as it did before.

The `PUT` signature binds length, checksum, content type, conditional
creation, and an S3 metadata digest of the authenticated subject and upload
metadata; completion recomputes that digest from the authenticated subject
and submitted metadata. An object id alone cannot authorise access or
change the destination, and no upload table or second signing secret is
needed. Destination routing and filesystem validation run again at install.

Only control constructs download URLs. The node accepts them over its
existing authenticated channel, checks the exact S3 host and key, rejects
redirects, streams the body under a size cap, verifies the hash, and
installs atomically through directory-relative operations
(`backend/src/uploads/install.rs`). Two concurrent imports bound node
resource use; a busy node returns an error. The import request has a
deadline below the ALB's 300-second idle timeout.

Uploads are foreground operations. A failure reports an error for manual
retry; paste dialogs retain their content and offer retry, explicit inline
paste, or cancel. There is no upload list, recovery UI, polling,
persistence, cross-page state, or automatic replay. If a completion
response is lost, the user checks the destination before retrying, with the
same overwrite behaviour as a new upload.

A stateless browser helper selects S3 only for the configured exact public
origin (`SULION_PUBLIC_UPLOAD_ORIGIN`); LAN origins keep multipart, and a
remote failure never falls back to multipart. Infrastructure is a private,
encrypted bucket with exact-origin CORS, 15-minute presigned grants, scoped
backend IAM, and a two-day lifecycle expiration that does the cleanup; a
configured `SULION_UPLOAD_BUCKET` enables the transport. The companion
`ahara-infra` change grants the deployer the bucket-policy permissions and
adds a structured `WAF_LFI_BODY` response to the existing LFI block action
so the UI can name that error. Nodes need no AWS credentials.

## Alternatives considered

- **Multipart through the public ALB and WAF.** The existing LAN path.
  Unusable remotely: the audit in `docs/security/external-audit-2026-09-28.md`
  records the managed size rule's 8 KiB threshold and that "S3 solves
  file-byte transport", and the plan's outcome is stated as working
  "without sending file bodies through the public ALB/proxy/application
  ingress".
- **WAF exclusions or a weaker rule for upload routes.** Rejected in the
  plan ("No WAF exclusion or weaker rule is introduced") and by the audit's
  statement that the body limit is an existing product constraint, "not a
  reason to disable LFI/XSS rules". The WAF is shared; loosening it for
  Sulion loosens it for every service behind the listener.
- **The durable upload subsystem.** The first design, with its migration,
  dispatcher, retry/reconciliation state machine, status APIs, global store,
  and uploads panel. Rejected by the user on 2026-09-25, as the plan
  records, in favour of visible errors and manual retry with no survival
  guarantee across browser closure, process restart, or lost responses.
  Single-operator uploads of files the user is looking at do not need
  delivery guarantees; S3 metadata and lifecycle already carry the state
  the subsystem would have duplicated.

## Consequences

- File bodies never touch the public application ingress, and WAF
  enforcement on every other request is unchanged.
- A lost completion response can leave an installed file the UI did not
  acknowledge; the user checks and retries. A hard process kill during
  install can leave a hidden temporary file; there is no restart cleanup.
- The browser can PUT only to the exact S3 origin, which the CSP later
  derived from `SULION_UPLOAD_BUCKET` and the backend region (ADR 0027)
  rather than from a separately configured value.
- The 8 KiB WAF body threshold still applies to JSON prompts and other
  request text; S3 staging solves file bytes only.
- The safer installer written for this path became the single upload
  writer when the legacy multipart writer's symlink escapes were found
  (ADR 0027, finding F02).
- Deployed CORS, signing, and a real public upload were not verified at
  acceptance; the plan was closed on 2026-10-01 with that gate skipped at
  the user's direction.

## References

- Deployment and operation: [`../deploy.md`](../deploy.md), "Remote file
  uploads"
- Published Sulion plan `0b162a4c-bfdd-4c08-824f-29c223326c49` and its
  simplification expansion `08bc8220-7a0b-4833-988b-5e51371dafbb`; the
  transfer contract and the recorded rejection are folded into this ADR
- Audit statements on WAF body limits:
  [`../security/external-audit-2026-09-28.md`](../security/external-audit-2026-09-28.md),
  "Availability constraints and accepted trust boundaries"
- Change record: `CHANGELOG.md`, Unreleased, "Remote uploads"
- Commit: `61a34ae` — Route remote file uploads through private S3 staging
