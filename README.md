# shuttlecraft

Session broker for Claude Code terminal sessions. Persistent PTYs with structured timeline scrollback, served to any LAN device via web UI.

- Architectural design: [`session-broker-design.md`](session-broker-design.md)
- Invariants and contributor guide: [`CLAUDE.md`](CLAUDE.md)

## Status

Early development. Backend and frontend MVPs landed; first Komodo deploy (issue #14) is the next verification step.

## Local development

Requires Rust (for the backend), pnpm + Node 24 (for the frontend), and a local Postgres for backend tests.

```bash
make ci              # full lint + test + build

# Backend
cd backend && cargo run                        # SHUTTLECRAFT_DB_URL must be set

# Backend integration tests (require a real Postgres)
docker run --rm -d --name shuttlecraft-test-db -p 55432:5432 \
  -e POSTGRES_PASSWORD=testpass -e POSTGRES_DB=shuttlecraft postgres:16
SHUTTLECRAFT_TEST_DB='postgres://postgres:testpass@localhost:55432/shuttlecraft' \
  cargo test --release -- --ignored

# Frontend
cd frontend && pnpm install && pnpm dev       # proxies /api and /ws to :8080
```

## First-run on TrueNAS

The service is deployed to TrueNAS as a Docker Compose stack via Komodo. One-time setup:

### 1. Provision the dataset

SSH into TrueNAS as root and run the bootstrap script from a checkout of this repo:

```bash
./scripts/truenas-bootstrap.sh
```

It creates `/tank/dev/shuttlecraft/{home,repos,postgres}` with the UID/GID ownership the container expects (1000:1000 for dev data, 999:999 for Postgres). Idempotent — safe to re-run.

Override the paths/UIDs via env vars if your TrueNAS layout differs:

```bash
SHUTTLECRAFT_DATASET=/mnt/tank/shuttlecraft \
SHUTTLECRAFT_DEV_UID=1001 SHUTTLECRAFT_DEV_GID=1001 \
  ./scripts/truenas-bootstrap.sh
```

### 2. Drop in credentials & config

- SSH keys for `git clone` → `/tank/dev/shuttlecraft/home/.ssh/` (chmod 0600 for private keys)
- Git identity → `/tank/dev/shuttlecraft/home/.gitconfig`
- Claude auth: copy your `~/.claude/.credentials.json` (or whatever Claude creates after `claude login`) into `/tank/dev/shuttlecraft/home/.claude/`
- GitHub token for `gh`, if you use it → `/tank/dev/shuttlecraft/home/.config/gh/hosts.yml`

These live in the dataset, not in the container image, so they persist across deploys.

### 3. Register the DB password in SSM

The Postgres sidecar reads its password from `/ahara/shuttlecraft/db-password`:

```bash
aws ssm put-parameter \
  --name /ahara/shuttlecraft/db-password \
  --type SecureString \
  --value "$(openssl rand -hex 24)"
```

### 4. Deploy via Komodo

The ahara shared CI workflow builds both images, pushes to GHCR, and calls the `deploy-truenas` action. Register the stack in Komodo (one-time) so it knows to accept deploys for the `shuttlecraft` project.

### 5. Verify

```bash
curl -sf http://<truenas-ip>:30080/health
# → {"status":"ok","db":"ok"}
```

Open `http://<truenas-ip>:30080/` in a browser, create a repo, spawn a session, and run `claude` inside. The `SessionStart` hook (pre-installed at `home/.claude/settings.json` by the bootstrap script) posts the correlation to the backend so the timeline knows which events belong to which PTY.

## Ingestion model

All JSONL reads happen in the backend ingester — the REST API and WebSocket paths query Postgres only. See `CLAUDE.md` for the full list of architectural invariants (including why this matters for test surface, format-drift tolerance, and future Codex support).

## License

MIT — see [`LICENSE`](LICENSE).
