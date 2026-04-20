# sulion — agent orientation

Session broker for Claude Code and Codex terminal sessions. Rust backend + React frontend + Postgres, deployed to TrueNAS via Komodo.

Read these before editing:

- [docs/architecture.md](docs/architecture.md) — shape, session model, **invariants**
- [docs/ingestion.md](docs/ingestion.md) — ingestion runtime boundary and split plan
- [docs/state-management.md](docs/state-management.md) — Zustand store rules + app command layer
- [docs/design.md](docs/design.md) — visual tokens, primitives, tooltip tiers
- [docs/development.md](docs/development.md) — make targets, test contracts, CI shape
- [docs/deploy.md](docs/deploy.md) — TrueNAS dataset layout and deploy flow
- [docs/e2e-coverage-plan.md](docs/e2e-coverage-plan.md) — Playwright suite shape and gaps

## Invariants — do not break

Full list in [docs/architecture.md](docs/architecture.md#invariants--do-not-break). Short form:

1. Only the ingester reads JSONL. REST / WS paths query Postgres.
2. The terminal pane lives outside React reconciliation. React never re-renders it on PTY bytes.
3. Ingester tolerates partial lines and unknown event types.
4. Shadow terminal emulator is fed continuously, including with no clients attached.
5. Ingester idempotency key is `(session_uuid, byte_offset)`.
6. Schema carries `parent_session_uuid NULL` from day one.

## Working rules

- Backend integration tests run through `make test-rust-integration` / `scripts/run-backend-integration-tests.sh`. Never `#[ignore]`; register new targets in the script.
- E2E is the single Playwright suite in `frontend/e2e/`, backed by `scripts/run-e2e-stack.mjs`. No in-browser MSW mock mode.
- Git staleness, invariants, and the dataset layout live in docs/; don't re-explain them in comments or commit messages.
- Codex and Claude share the same canonical event schema. When editing ingest code, assume both agents flow through it.

## Companion doc

[`CLAUDE.md`](CLAUDE.md) is the Claude Code–facing twin of this doc — keep the two in sync when the content applies equally.
