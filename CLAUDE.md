# sulion — agent orientation

Session broker for Claude Code and Codex terminal sessions. Rust backend + React frontend + Postgres, deployed to TrueNAS via Komodo.

## ⛔ Never `git push` without the user's explicit say-so

A push to `main` triggers a Komodo **redeployment of Sulion, which kills every
active agent session — including your own.** This is the single most destructive
thing you can do here. Commit when asked; **push only when the user expressly
tells you to push _this_ change.** A "push" in an earlier turn is never standing
permission. When in doubt, stop and ask. Never force-push to undo a push without
explicit approval.

## Scope default — LAN only

Treat Sulion and every operational path in this repository as LAN-only unless
the user explicitly asks for public access or public-edge work in the current
request. Do not infer that ALB, WAF, public DNS, VPN ingress, or `ahara-infra`
changes are in scope merely because those paths exist in deployment docs.
Diagnose and fix the LAN path first; ask before expanding a task into public
infrastructure.

Read these before editing:

- [docs/architecture.md](docs/architecture.md) — shape, session model, **invariants**
- [docs/ingestion.md](docs/ingestion.md) — ingestion runtime boundary and split plan
- [docs/state-management.md](docs/state-management.md) — Zustand store rules + app command layer
- [docs/design.md](docs/design.md) — visual tokens, primitives, tooltip tiers
- [docs/development.md](docs/development.md) — make targets, test contracts, CI shape
- [docs/deploy.md](docs/deploy.md) — TrueNAS dataset layout and deploy flow
- [docs/secrets.md](docs/secrets.md) — brokered secrets, wrappers, trust boundary
- [docs/e2e-coverage-plan.md](docs/e2e-coverage-plan.md) — Playwright suite shape and gaps
- [docs/code-intel.md](docs/code-intel.md) — agent-facing structural code navigation contract

## Invariants — do not break

Full list in [docs/architecture.md](docs/architecture.md#invariants--do-not-break). Short form:

1. Only the ingester reads JSONL. REST / WS paths query Postgres.
2. The terminal pane lives outside React reconciliation. React never re-renders it on PTY bytes.
3. Ingester tolerates partial lines and unknown event types.
4. Shadow terminal emulator is fed continuously, including with no clients attached.
5. Ingester idempotency key is `(session_uuid, byte_offset)`.
6. Schema carries `parent_session_uuid NULL` from day one.
7. The node owns no PTY masters — shells live in the devenv server, which is never a compose service.

## Working rules

- Backend integration tests run through `make test-rust-integration` / `scripts/run-backend-integration-tests.sh`. Never `#[ignore]`; register new targets in the script.
- E2E is the single Playwright suite in `frontend/e2e/`, backed by `scripts/run-e2e-stack.mjs`. No in-browser MSW mock mode.
- Git staleness, invariants, and the dataset layout live in docs/; don't re-explain them in comments or commit messages.
- Codex and Claude share the same canonical event schema. When editing ingest code, assume both agents flow through it.
- In Sulion PTYs, run `sulion-code help` before structural code navigation.
- `sulion plan` publishes a durable user-facing phase summary when useful; it
  complements rather than replaces an agent's detailed internal plan. See
  `docs/plans.md`.

## Companion doc

[`AGENTS.md`](AGENTS.md) carries the same orientation for Codex and other non-Claude agents — keep the two in sync when the content applies equally.
