# sulion

Session broker for Claude Code terminal sessions. Hosts persistent PTYs on a TrueNAS server and provides a web UI with structured timeline scrollback.

See `session-broker-design.md` for the full architectural design and rationale.

## Stack

- **Backend**: Rust (`axum`, `tokio`, `sqlx`, `portable-pty`, `vt100`, `notify`) — `backend/`
- **Frontend**: TypeScript + React + Vite, `xterm.js`, `react-virtuoso` — `frontend/`
- **Database**: PostgreSQL 16 (sidecar in compose stack, not shared platform RDS)
- **Deploy**: Docker Compose via Komodo on TrueNAS (ahara standard)

## Ahara ecosystem integration

Sulion is a standard TrueNAS/Komodo deploy. The pattern matches `nas-sonarqube`:

- **Shared TrueNAS Postgres** at `192.168.66.3:5432`. Registered in `ahara-infra/infrastructure/terraform/services/db-migrate-truenas.tf` under `var.truenas_db_projects`. The `ahara-db-migrate-truenas` Lambda creates the DB + app role and publishes credentials to `/ahara/truenas-db/sulion/{username,password}` in SSM.
- **Deploy-truenas action auto-creates the Komodo stack** on first push (tolerant of already-exists). No manual UI setup.
- **No `infrastructure/terraform/`** in this repo. Nothing to apply from the project's own state. Cross-repo registration only: the `project-sulion.tf` file in ahara-infra's control layer grants the deployer role `terraform-state` + `komodo-deploy` policies.
- **No reverse-proxy route** for MVP. LAN-only via WireGuard, bound to `192.168.66.3:30080`. Add a `reverse_proxy_routes` entry in ahara-network with `auth = "jwt-validation"` when public exposure is wanted.

Sulion-specific divergence from typical ahara services:

- **Thick backend container.** Carries `git`, `openssh-client`, `node`, `bash` — the PTY shell is the product, not an accident. See `backend/Dockerfile`.
- **Dataset-backed workbench.** `/tank/dev/sulion/` on TrueNAS is bind-mounted into the backend container at `/home/dev/`. All user state (SSH keys, Claude creds, installed tools under `.local/`) lives in this dataset and survives image rebuilds.

## Dataset-backed workbench (TrueNAS)

Single dataset at `/mnt/apps/apps/sulion`, bind-mounted directly as `/home/dev` in the backend container. The dataset root **is** the dev user's home — no `home/` + `repos/` split, no subpath layout to prepare.

The container's entrypoint (`backend/entrypoint.sh`, runs as the `dev` user) idempotently creates the expected subtree on first boot:

- `~/.claude/` with a default `settings.json` wiring the SessionStart hook
- `~/.ssh/` at `chmod 0700`
- `~/.local/bin/`, `~/.config/gh/`, `~/repos/`

TrueNAS operator's whole bootstrap: `zfs create apps/apps/sulion && chown 7321:7321 /mnt/apps/apps/sulion`. No repo-side bootstrap script.

**UID/GID is 7321** — deliberately unusual to avoid the 1000-series collision that most consumer container images cause. `backend/Dockerfile` pins it via `DEV_UID` / `DEV_GID` build args; the dataset must be chowned to match.

## Architectural invariants — do not break

1. **Only the ingester reads JSONL.** REST handlers and WebSocket event pushes query Postgres. Never `fs::read` the `~/.claude/projects/` files from the request path.
2. **The terminal pane lives outside React's reconciliation.** Mount `xterm.js` imperatively; pipe WebSocket bytes directly. React must not re-render the terminal container in response to PTY data.
3. **Ingester must tolerate partial lines and unknown event types.** Partial lines: only commit on trailing `\n`. Unknown types: log and skip, do not crash. JSONL format is not a stable public API.
4. **Shadow terminal emulator is fed continuously**, including while no clients are attached. Otherwise snapshot-on-reconnect lags.
5. **Ingester idempotency key:** `(session_uuid, byte_offset)`. JSONL is append-only, so byte offset is stable.
6. **Schema includes `parent_session_uuid NULL`** from day one. Cheap now; avoids a migration when compaction UI arrives.

## Session correlation

Backend injects `SULION_PTY_ID=<pty_id>` into the PTY shell environment. A Claude Code `SessionStart` hook reads that env var and posts `{pty_id, claude_session_uuid}` to `/run/sulion/correlate.sock`. The backend records the association. When the user starts a new Claude session in the same PTY, the hook fires again and updates the current-claude-session pointer.

The hook script ships in this repo at `scripts/claude-hooks/session-start.sh`. Install it once per dataset:

```bash
mkdir -p /tank/dev/sulion/home/.claude
cat > /tank/dev/sulion/home/.claude/settings.json <<'EOF'
{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "/opt/sulion/hooks/session-start.sh" }] }
    ]
  }
}
EOF
```

The container image places the hook at `/opt/sulion/hooks/session-start.sh`. Hook failure is silent — if the socket is gone, Claude continues without correlation (the session is still ingested from the JSONL file; only the pty↔claude-session link is missing).

## Local development

```
make ci          # run the full CI check locally
make lint-rust   # clippy
make fmt-rust    # fmt --check
make test-rust   # backend unit/non-DB tests
make test-rust-integration   # Postgres-backed backend integration suite
make lint-ts     # eslint
make typecheck-ts
make test-ts     # vitest run
```

Backend integration contract: Postgres-backed tests live in `backend/tests/*_integration.rs`,
are gated with `#![cfg(feature = "integration-tests")]`, and run through
`scripts/run-backend-integration-tests.sh` / `make test-rust-integration`. Do not add
`#[ignore]` to those tests. When you add a new backend integration target, register it in the
script so the harness remains the single supported path.

## CI

`.github/workflows/ci.yml` is a minimal caller of the shared ahara workflow at `chris-arsenault/ahara/.github/workflows/ci.yml@main`. The shared workflow handles lint/test/build/Docker push/Komodo deploy based on `platform.yml`.
