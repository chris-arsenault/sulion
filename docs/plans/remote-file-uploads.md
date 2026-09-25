# Remote uploads through S3

## Outcome and scope

Explorer uploads, pasted text files and clipboard images work on
`https://sulion.services.ahara.io` without sending file bodies through the public
ALB/proxy/application ingress. WAF protections remain in place. LAN uses the
existing multipart endpoints. The existing 50 MiB limit and overwrite behavior
remain.

On 2026-09-25 the user explicitly rejected the initial durable-delivery design.
Remove its database migration, dispatcher, retry/reconciliation state machine,
status APIs, global browser store and uploads panel. Uploads are foreground
operations with visible errors and manual retry. No survival guarantee across
browser closure, process restarts or lost responses.

The user authorized committing and pushing the simplified implementation on
2026-09-25. Publish the infrastructure prerequisite before Sulion.

## Transfer

1. Browser sends small authenticated metadata to `POST /api/uploads`: repository
   or workspace, directory, filename, byte count and SHA-256.
2. Control validates metadata and resolves the destination node, then returns a
   generated UUID and a constrained presigned S3 PUT.
3. Browser PUTs the raw file directly to S3 without Sulion cookies or credentials.
4. Browser sends the same metadata to `POST /api/uploads/:id/complete`.
   Control checks the object's size, checksum and signed metadata binding, then
   asks the node to download and install it within the same request.
5. Node returns the existing installed `{path, size}` result. The current UI
   inserts the path or refreshes the file list.

The PUT signature binds length, checksum, content type, conditional creation,
and an S3 metadata digest of the authenticated subject and upload metadata.
Completion compares that digest using the authenticated subject and submitted
metadata. An object ID alone cannot authorize access or change the destination.
This reuses S3 metadata without an upload database or another signing secret.
Destination routing and filesystem validation run again at installation.

Only control constructs download URLs. The node accepts them over its existing
authenticated channel, checks the exact S3 host/key and rejects redirects.
Downloads are streamed, capped, hashed and installed atomically using
directory-relative file operations. Two concurrent imports bound node resource
use; a busy node returns an error instead of queuing work.

The import request has a bounded deadline below the public ALB's checked-in
300-second idle timeout. Other node requests keep their existing timeout.
There is no automatic replay: if a response is lost, the user checks the
destination and may retry, with the same overwrite behavior as a new upload.
A failed download does not replace the destination. A hard process kill can
leave a hidden temporary file; there is no restart cleanup service.

## Browser behavior

A stateless upload helper selects S3 only for the configured exact public
origin. Remote failures never fall back to multipart. Paste dialogs retain
content after failure and offer manual retry, explicit inline paste, or cancel.
The selected session's destination is captured when pasting; successful paths
are inserted only while that consumer is still mounted.

No upload list, recovery UI, polling, persistence or cross-page state.
Each manual retry starts a fresh transfer. Ordinary Explorer and clipboard
interactions remain the entry points.

## Infrastructure and deployment

Keep the private, encrypted staging bucket, exact-origin CORS, 15-minute
presigned PUT/GET grants, scoped IAM and two-day lifecycle expiration.
S3 performs object cleanup; no Lambda, queue or scheduler is added.
The bucket configuration enables staging; remove the extra enablement flag.

The companion ahara-infra change grants the deployer the required narrowly
scoped bucket-policy/ownership permissions and adds a structured response to
the existing LFI Block action. No WAF exclusion or weaker rule is introduced.

Publish the ahara-infra provisioning grant before Sulion infrastructure.
After publication/deployment, the implementing agent verifies actual S3 CORS,
signatures and one upload through the public hostname. This is ordinary
deployment verification owned by the implementer, not a manual acceptance
project for the user. Local tests cannot establish those deployed facts.

## Execution and evidence

Root plan: `0b162a4c-bfdd-4c08-824f-29c223326c49`.
Simplification expansion: `08bc8220-7a0b-4833-988b-5e51371dafbb`.

1. Replace durable import with foreground grant/PUT/install and remove recovery
   UI and state. Update the existing implementation and documentation together.
2. Verify signature/ownership binding, request limits, streaming installation,
   checksum failure, destination safety, local transport and paste failures.
   Run focused backend integration, frontend tests, typecheck, lint and config
   checks; review the reduced two-repository diff.

Verification of the simplified implementation, 2026-09-25:

- 304 Rust unit tests and both structural checks passed. The timeout change
  initially exceeded an existing impl block's line limit; moving the timeout
  definition onto the request type resolved it.
- 32 isolated integration tests passed: upload installation, node protocol and
  workspace suites. Upload coverage includes metadata limits, checksums,
  preserved destination bytes on failure, temporary-file cleanup, symlink
  rejection, workspace routing and deletion, and arbitrary-URL rejection.
- 380 frontend tests passed, including direct S3 request boundaries, no public
  multipart fallback or automatic retry, and retention of failed pasted text.
- All four real-stack Chromium terminal tests passed, including text/image
  paste and exact uploaded bytes. Two initial attempts exceeded the harness's
  300-second startup limit while building images; the cached run passed.
  This exercises the LAN browser path, not a deployed AWS endpoint.
- TypeScript, final all-target Clippy, Rust formatting, focused ESLint,
  Compose validation, Terraform formatting and both repository whitespace
  checks passed. ESLint retains an existing TimelinePane complexity warning.

Local verification is complete. Publication is authorized; deployed AWS
verification remains outstanding. The two companion ahara-infra changes are
unchanged from their previously validated scope. Normal deployment verification
belongs to the implementing agent.

AWS contracts: [PutObject](https://docs.aws.amazon.com/AmazonS3/latest/API/API_PutObject.html),
[HeadObject](https://docs.aws.amazon.com/AmazonS3/latest/API/API_HeadObject.html),
[conditional writes](https://docs.aws.amazon.com/AmazonS3/latest/userguide/conditional-writes.html).
