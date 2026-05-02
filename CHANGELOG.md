# Changelog

All notable user-visible changes to Sulion are recorded here.

## Unreleased

### Runtime container and toolchain

- Rebased the backend/PTY image from Debian Trixie to Rocky Linux 10 to keep a glibc 2.39 runtime while using Rocky's `shadow-utils`/`newuidmap` behavior for nested rootless Podman without `SYS_ADMIN`.
- Translated the backend image package setup from `apt` to `dnf`, with EPEL/CRB enabled and Rocky package names for Podman, build tools, GitHub CLI, PostgreSQL client tooling, and shell utilities.
- Kept the existing PTY tool surface on the Rocky image, including Rust, .NET 8, .NET 10.0.100, Terraform, DuckDB CLI/Python binding, Node/pnpm, Python helpers, `uv`, `awscli2`, `git-lfs`, and the `docker` to Podman shim.
- Changed PTY `python3` to a Python 3.12 shim under `/usr/local/bin` while leaving Rocky's system Python path intact for `dnf`.
- Changed backend startup so the API listener binds after migrations and orphan reconciliation, while derived transcript repair runs only when `ingest_projection_versions` is behind.
- Changed transcript repair to preserve source `events` rows and rebuild derived canonical/timeline tables from existing Postgres payloads instead of deleting data and relying on JSONL replay.
- Fixed canonical-block repair so it skips already-populated events instead of reprocessing historical Codex events on every backend restart.

## v1.0.0 - 2026-05-02

### Security and secrets

- Added a separate Sulion secret broker service with encrypted-at-rest env bundle storage, isolated broker database usage, and a broker-only master key.
- Added per-PTY credential registration using signed secret-use requests, nonce replay protection, and revocation on PTY shutdown.
- Reworked credential consumption down to the two supported modes: `with-cred` for env injection and the Sulion `aws` wrapper for AWS CLI access.
- Changed secret reads so the UI receives metadata and env key names, not raw stored secret values. Existing values can be overwritten or preserved without being revealed.
- Added TTL-based per-terminal grants, active-grant revocation, conflict detection for overlapping `with-cred` env keys, and context-menu grant workflows.

### Timeline and ingestion

- Replaced full timeline polling with lightweight summary responses plus per-turn detail endpoints. The frontend now caches detail for older turns while refetching the active turn when its summary changes.
- Added repo/session turn-detail routes and repo-membership validation for repo-scoped timeline detail requests.
- Made timeline projection updates incremental for direct append cases instead of rebuilding the entire session projection every tick.
- Added batched dirty-transcript detection so ingest can stat files and load committed offsets in bulk before deciding what to read.
- Changed startup projection backfill to rebuild only sessions missing projection rows.
- Fixed admin reindex so transcript replay preserves correlated terminal/session associations.

### Secrets UI and UX

- Added a dedicated Secrets work-area tab for creating and editing env-bundle secrets.
- Moved grant enablement out of the Secrets tab and into terminal/session right-click menus: Secrets -> Enable secret -> tool -> TTL.
- Added active-secret context-menu entries that show remaining TTL and revoke immediately when clicked.
- Added frontend state and tests for secret metadata, grant refresh, context-menu conflicts, and broker write responses.

### Runtime container and toolchain

- Added `sudo`, `git-lfs`, Terraform, DuckDB CLI, DuckDB Python bindings, `uv`/`uvx`, `click`, `Pillow`, and `rembg[cpu]` to the PTY image.
- Added .NET SDK support for both .NET 8 and .NET 10.0.100 so SDK resolution works for repos pinned to either SDK.
- Standardized Rust tooling in the image so `cargo`, `rustfmt`, and `cargo clippy` are available on the default shell `PATH`.
- Added `/opt/sulion/docs/toolset.md`, baked into the image outside workspace bind mounts, documenting the tools and Sulion-specific wrapper behavior available to agents.

### Infrastructure and deployment

- Updated compose wiring for the broker registration token and per-PTY secret key flow.
- Updated Terraform outputs/platform registration and secret path registration for the broker integration.
- Added broker migration support for PTY credential registration and nonce tracking.

### Documentation and tests

- Added and refreshed docs for architecture, secrets, user workflow, deployment/tooling behavior, and current user-visible feature history.
- Added backend integration coverage for incremental projection, dirty-file filtering, reindex correlation preservation, and PTY association restoration.
- Added frontend unit coverage for timeline summary/detail behavior, secret context menus, and Secrets tab behavior.
- Added a real-stack Playwright secrets suite covering the supported secrets UX.
