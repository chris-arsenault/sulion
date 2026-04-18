# shuttlecraft

Session broker for Claude Code terminal sessions. Hosts persistent PTYs on a TrueNAS server and provides a web UI with structured timeline scrollback.

See `session-broker-design.md` for the full architectural design and rationale.

## Stack

- **Backend**: Rust (`axum`, `tokio`, `sqlx`, `portable-pty`, `vt100`, `notify`) — `backend/`
- **Frontend**: TypeScript + React + Vite, `xterm.js`, `react-virtuoso` — `frontend/`
- **Database**: PostgreSQL 16 (sidecar in compose stack, not shared platform RDS)
- **Deploy**: Docker Compose via Komodo on TrueNAS (ahara standard)

## Ahara ecosystem exceptions

Shuttlecraft deliberately diverges from ahara defaults in a few places, with rationale in the design doc:

- **Sidecar Postgres** instead of shared RDS or shared TrueNAS Postgres. The DB is a query-cache over local JSONL; it has no cross-service consumers. Avoids cross-repo terraform coordination.
- **No `infrastructure/terraform/`** for MVP. No ALB route, no Cognito client, LAN-only. Add when public exposure becomes a requirement (route in `ahara-network` with `jwt-validation`).
- **Thick backend container.** The container runs interactive PTY shells; carries `git`, `openssh-client`, `node`, etc. Normal ahara services are thin; this one is by design a dev workbench host.

## Dataset-backed workbench (TrueNAS)

The backend bind-mounts `/tank/dev/shuttlecraft/` from TrueNAS:

```
/tank/dev/shuttlecraft/
  home/    → /home/dev/ in container
  repos/   → /home/dev/repos/ in container
  postgres/ → postgres sidecar data
```

UID/GID of the container's `dev` user must match dataset ownership. All dev state (Claude creds, SSH keys, gitconfig, user-installed tools under `~/.local/`) lives in the dataset and persists across image rebuilds.

## Architectural invariants — do not break

1. **Only the ingester reads JSONL.** REST handlers and WebSocket event pushes query Postgres. Never `fs::read` the `~/.claude/projects/` files from the request path.
2. **The terminal pane lives outside React's reconciliation.** Mount `xterm.js` imperatively; pipe WebSocket bytes directly. React must not re-render the terminal container in response to PTY data.
3. **Ingester must tolerate partial lines and unknown event types.** Partial lines: only commit on trailing `\n`. Unknown types: log and skip, do not crash. JSONL format is not a stable public API.
4. **Shadow terminal emulator is fed continuously**, including while no clients are attached. Otherwise snapshot-on-reconnect lags.
5. **Ingester idempotency key:** `(session_uuid, byte_offset)`. JSONL is append-only, so byte offset is stable.
6. **Schema includes `parent_session_uuid NULL`** from day one. Cheap now; avoids a migration when compaction UI arrives.
7. **Postgres is stack-local.** Do not migrate it to shared RDS without re-reading the rationale in the design doc.

## Session correlation

Backend injects `SHUTTLECRAFT_PTY_ID=<pty_id>` into the PTY shell environment. A Claude Code `SessionStart` hook reads that env var and posts `{pty_id, claude_session_uuid}` to `/run/shuttlecraft/correlate.sock`. The backend records the association. When the user starts a new Claude session in the same PTY, the hook fires again and updates the current-claude-session pointer.

The hook script ships in this repo at `scripts/claude-hooks/session-start.sh`. Install it once per dataset:

```bash
mkdir -p /tank/dev/shuttlecraft/home/.claude
cat > /tank/dev/shuttlecraft/home/.claude/settings.json <<'EOF'
{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "/opt/shuttlecraft/hooks/session-start.sh" }] }
    ]
  }
}
EOF
```

The container image places the hook at `/opt/shuttlecraft/hooks/session-start.sh`. Hook failure is silent — if the socket is gone, Claude continues without correlation (the session is still ingested from the JSONL file; only the pty↔claude-session link is missing).

## Local development

```
make ci          # run the full CI check locally
make lint-rust   # clippy
make fmt-rust    # fmt --check
make test-rust   # cargo test
make lint-ts     # eslint
make typecheck-ts
make test-ts     # vitest run
```

## CI

`.github/workflows/ci.yml` is a minimal caller of the shared ahara workflow at `chris-arsenault/ahara/.github/workflows/ci.yml@main`. The shared workflow handles lint/test/build/Docker push/Komodo deploy based on `platform.yml`.
