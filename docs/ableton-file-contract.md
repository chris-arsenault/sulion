# Ableton ↔ Sulion file contract (proposed/spec)

This spec is authored by the **ableton-extensions** repo (the client) for the Sulion
backend agent. The Ableton extensions transfer clips as **Standard MIDI Files** over the
generic per-repo file endpoints, authenticated with a device token (see the pairing flow
in `ableton-extensions/docs/auth.md`).

## As built — upload (no change requested)

`POST /api/repos/:name/ingest?path=<repo-relative-path>` — device `Authorization: Bearer`,
`Content-Type: application/octet-stream`, raw bytes (≤50 MiB) → `200 {path, bytes}`.
The extension writes `.mid` files under a configurable repo (client env `SULION_REPO`,
default `ableton`) at `clips/<clip-name>.mid`. This already works; documented here only
so both sides share one contract.

## As built — device-authed raw download

Pulling a Sulion-generated clip **back into Live** reads a `.mid` file with the device
token. The existing `GET /api/repos/:name/file` is Cognito-only and returns
`content: null` for binary, so a dedicated endpoint serves this — implemented exactly as
proposed:

`GET /api/repos/:name/raw?path=<repo-relative-path>`

- **Auth:** `Authorization: Bearer <device-token>` (same `require_device_token` middleware as ingest).
- **Response `200`:** the raw file bytes, `Content-Type: application/octet-stream`.
- **`400`** — empty/invalid path or traversal (`..`/absolute/symlink-escape).
- **`401`** — token missing/expired/revoked. **`404`** — no file (or non-file) at `path`.
- No size cap beyond the existing repo limits.

Source in `../sulion`: `backend/src/api/repo_routes.rs` (`get_repo_raw`, `RawQuery`),
wired in `backend/src/api/mod.rs` under the device-token layer, covered by
`backend/tests/device_integration.rs` (round-trip, 404, 401, read-side traversal).

The client (`ableton-extensions`) can build its pull-back extension against this shape.
If a different verb/path/response is preferred, edit this file and flag it — this doc
stays the single source of truth for the file contract.
