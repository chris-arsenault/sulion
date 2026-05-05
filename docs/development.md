# Development

## Prerequisites

- Rust (backend)
- Node 24 + pnpm (frontend)
- Docker or an existing Postgres 16 (backend integration tests)

## Daily commands

```bash
make ci                      # full lint + unit + backend integration
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

## Backend integration test contract

Postgres-backed tests live in `backend/tests/*_integration.rs`, gated with `#![cfg(feature = "integration-tests")]`, and run through `scripts/run-backend-integration-tests.sh` (also `make test-rust-integration`).

- The harness enables the `integration-tests` Cargo feature, runs each integration target one at a time with `--test-threads=1`, and auto-starts an ephemeral `docker.io/library/postgres:16` container via Docker when `SULION_TEST_DB` is unset.
- In Sulion PTYs, the runner automatically attaches that container to the `sulion` Docker network, and tests connect to the container name on port `5432`; no Docker socket or host port discovery is required in the PTY.
- Do not mark backend integration tests `#[ignore]`. When adding a new target, register it in the script so the harness stays the single supported path.

Override the DB:

```bash
SULION_TEST_DB='postgres://postgres:testpass@127.0.0.1:55432/sulion' \
  make test-rust-integration
```

## E2E

Real stack + Postgres + seeded ingest data via `scripts/run-e2e-stack.mjs`. Specs live in `frontend/e2e/`. Current coverage and the prioritized next-test list live in [`e2e-coverage-plan.md`](e2e-coverage-plan.md).

## CI

`.github/workflows/ci.yml` is a minimal caller of the shared ahara workflow at `chris-arsenault/ahara/.github/workflows/ci.yml@main`. Lint / test / build / Docker push / Komodo deploy are driven by `platform.yml`.
