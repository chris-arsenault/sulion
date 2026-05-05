# Changelog

All notable user-visible changes to Sulion are recorded here.

## v1.2.0 - 2026-05-05

### Agent control and monitoring

- Added an Escape-based interrupt action beside the timeline prompt Send button so Codex/Claude sessions can be interrupted without opening the full terminal.
- Reworked the Monitor tab into a reading-first layout where recent assistant output owns the main space and user prompts are shown as compact context.

### Workspace management

- Added sidebar workspace management under each repo. Isolated workspaces can now be resumed into a new PTY, opened in a workspace-scoped diff, deleted, or force-deleted from the existing repo navigation surface.
- Added backend isolated-workspace deletion through `DELETE /api/workspaces/:id`, with live/orphaned session protection, dirty-worktree force checks, Git worktree removal, branch cleanup, and database state cleanup.

### Runner and integration tests

- Constrained runner-launched containers to the internal `sulion` Docker network and removed caller-controlled network selection from the Docker shim contract.
- Added `docker compose` and `docker-compose` shim support that maps Compose's default network to the external `sulion` network.
- Updated the backend integration harness to use the runner-exposed network path in Sulion PTYs while preserving native Docker host-port mapping outside the runner.

### Bug fixes

- Fixed timeline prompt submission so backend-injected prompts send Enter as a separate PTY input chunk instead of leaving text waiting at the agent prompt.
- Fixed isolated workspace deletion when the worktree directory is already missing but Git still has a registered worktree entry.
- Fixed the runner image so the Docker CLI and Compose plugin are installed and validated at image build time.
- Fixed the frontend nginx `/broker/*` proxy so browser secrets management routes are forwarded to the broker again while retaining Docker DNS re-resolution.
- Corrected the workspace dataset ownership contract so TrueNAS/ZFS bind mounts remain host-owned by `7321:7321` instead of relying on container-side ownership repair.

### Secrets and deployment

- Updated architecture, deployment, development, and toolset docs for workspace cleanup, the runner network boundary, Compose shim behavior, integration harness networking, and the secrets proxy fix.

## v1.1.0 - 2026-05-03

### Workspace isolation and agent flow

- Added Sulion-managed workspaces so PTY sessions can bind either to the canonical repo checkout or to an isolated Git worktree branch.
- Added workspace metadata on sessions, workspace-scoped file/diff/dirty/status APIs, and PTY environment variables plus `sulion workspace status` so agents can tell whether they are in `main` or an isolated worktree.
- Updated session creation so agent sessions default to isolated worktrees while still allowing explicit main-working-tree sessions.
- Added workspace-aware frontend routing for file tabs, diff tabs, file trace context menus, and session/sidebar indicators.
- Added first-class Claude/Codex launch support with `cl`/`co` executable shims, backend runtime metadata, and prompt injection from the timeline surface.

### Timeline and monitor UI

- Added the Monitor work-area tab for recent assistant output across active sessions.
- Added an input-only timeline prompt bar that can send prompts to a running agent without using the full terminal pane.
- Extended timeline/session metadata to surface agent runtime and transcript-reported model/context information.

### Container runner

- Replaced the old local Docker/Podman shim with a separate `runner` service that owns the host Docker socket and exposes a constrained command broker to PTYs.
- Added a PTY-visible `docker` wrapper that forwards allowed Docker commands to the runner with Sulion labels, resource defaults, and policy checks.
- Wired the runner to use the same canonical repo and isolated workspace mounts so `docker build .` works from either checkout.

### Runtime container and toolchain

- Rebased the backend/PTY image from Debian Trixie to Rocky Linux 10 to keep a glibc 2.39 runtime while using Rocky's `shadow-utils`/`newuidmap` behavior for nested rootless Podman without `SYS_ADMIN`.
- Translated the backend image package setup from `apt` to `dnf`, with EPEL/CRB enabled and Rocky package names for Podman, build tools, GitHub CLI, PostgreSQL client tooling, and shell utilities.
- Kept the existing PTY tool surface on the Rocky image, including Rust, .NET 8, .NET 10.0.100, Terraform, DuckDB CLI/Python binding, Node/pnpm, Python helpers, `uv`, `awscli2`, `git-lfs`, and the Sulion `docker` runner wrapper.
- Changed PTY `python3` to a Python 3.12 shim under `/usr/local/bin` while leaving Rocky's system Python path intact for `dnf`.

### Bug fixes

- Fixed backend startup so the API listener binds after migrations and orphan reconciliation, while derived transcript repair runs only when `ingest_projection_versions` is behind.
- Fixed transcript repair so it preserves source `events` rows and rebuilds derived canonical/timeline tables from existing Postgres payloads instead of deleting data and relying on JSONL replay.
- Fixed canonical-block repair so it skips already-populated events instead of reprocessing historical Codex events on every backend restart.
- Fixed a backend boot crash on deployed databases by restoring the original checksum for the already-applied canonical-block migration.

### Deployment and documentation

- Added the `runner` image/service to platform and compose wiring.
- Added a dedicated `/home/dev/workspaces` dataset/mount for Sulion-created worktrees.
- Updated agent-facing toolset docs, architecture docs, deployment docs, and state-management docs for workspaces, runner behavior, and the new PTY tool surface.

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

### Bug fixes

- Fixed admin reindex so transcript replay preserves correlated terminal/session associations.

### Infrastructure and deployment

- Updated compose wiring for the broker registration token and per-PTY secret key flow.
- Updated Terraform outputs/platform registration and secret path registration for the broker integration.
- Added broker migration support for PTY credential registration and nonce tracking.

### Documentation and tests

- Added and refreshed docs for architecture, secrets, user workflow, deployment/tooling behavior, and current user-visible feature history.
- Added backend integration coverage for incremental projection, dirty-file filtering, reindex correlation preservation, and PTY association restoration.
- Added frontend unit coverage for timeline summary/detail behavior, secret context menus, and Secrets tab behavior.
- Added a real-stack Playwright secrets suite covering the supported secrets UX.
