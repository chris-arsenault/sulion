# Development

## Prerequisites

- Rust (backend)
- Node 24 + pnpm (frontend)
- Docker or an existing Postgres 16 (backend integration tests)

## Daily commands

```bash
make ci                      # fast lint + unit/typecheck checks; no integration
make validate-deploy         # render every currently supported Compose role
make validate-nix            # evaluate the NixOS host and VM test
make test-nix                # build and run the NixOS VM acceptance test
make lint-rust               # clippy
make fmt-rust                # fmt --check
make test-rust               # backend unit / non-DB tests
make test-rust-integration   # Postgres-backed integration suite
make lint-ts                 # eslint
make typecheck-ts            # tsc --noEmit
make test-ts                 # vitest
make e2e                     # Playwright (real stack + seeded ingest)
make e2e-install             # one-time: playwright install chromium
```

## Running the services

```bash
# Backend (needs SULION_DB_URL)
cd backend && cargo run

# Frontend (proxies /api and /ws to :8080)
cd frontend && pnpm install && pnpm dev
```

The portable Compose bundle and role overlays are documented in
[`deploy/README.md`](../deploy/README.md). `compose.yaml` is the production
TrueNAS control-plane selection. Add `deploy/compose.standalone.yaml` for a
combined generic-Linux runtime, or `deploy/compose.dedicated.yaml` for the
node-only NixOS runtime. `deploy/compose.truenas-standalone.yaml` is the
single-file Komodo entry point for combined TrueNAS host operation; selecting
it requires changing only `truenas_compose_path` in `platform.yml`.

## PTY Postgres

In a Sulion PTY, use one managed workspace Postgres for repo tests:

```bash
sulion postgres -- cargo test
```

`sulion postgres -- <command>` creates or reuses a workspace-scoped Postgres 16
container, injects `DATABASE_URL`, `TEST_DATABASE_URL`, and `PG*` variables into
the command, and leaves the container running for the next test run. Use
`sulion postgres --restart -- <command>` for a clean database, or
`sulion postgres --temp -- <command>` for a one-off database removed after the
command exits.

## Backend integration test contract

Postgres-backed tests live in `backend/tests/*_integration.rs`, gated with `#![cfg(feature = "integration-tests")]`, and run through `scripts/run-backend-integration-tests.sh` (also `make test-rust-integration`).

- The harness enables the `integration-tests` Cargo feature, runs each integration target one at a time with `--test-threads=1`, and auto-starts an ephemeral `docker.io/library/postgres:16` container via Docker when `SULION_TEST_DB` is unset.
- The container's port is always published, and the harness probes how it is reachable rather than inferring it from which `docker` binary is on `PATH`. A reachable published port wins and tests connect to `127.0.0.1` on the mapped port; otherwise the harness falls back to the container name on port `5432` for callers that share a Docker network with it. If neither answers it fails with both addresses named, rather than handing the tests an address that cannot resolve.
- Do not mark backend integration tests `#[ignore]`. When adding a new target, register it in the script so the harness stays the single supported path.
- `node_protocol_integration` exercises real WebSocket pairing/authentication,
  reconnect and boot reconciliation, direct loopback requests, control
  replacement with a live PTY, and filesystem escape rejection. Do not replace
  those behaviors with serialized-message or source-text assertions.

Override the DB:

```bash
SULION_TEST_DB='postgres://postgres:testpass@127.0.0.1:55432/sulion' \
  make test-rust-integration
```

## E2E

Real stack + Postgres + seeded ingest data via `scripts/run-e2e-stack.mjs`.
Specs live in `frontend/e2e/`; the meta-repository spec creates and removes its
own grouping and collection PTY while reusing the seeded repos. Current
coverage and the prioritized next-test list live in
[`e2e-coverage-plan.md`](e2e-coverage-plan.md).

## CI

`.github/workflows/ci.yml` is a minimal caller of the shared ahara workflow at `chris-arsenault/ahara/.github/workflows/ci.yml@main`. Lint / test / build / Docker push / Komodo deploy are driven by `platform.yml`.
After that job succeeds on `main`, the caller advances `node-release` to the
same commit for the dedicated host's ordered NixOS-and-application pull deployer.
