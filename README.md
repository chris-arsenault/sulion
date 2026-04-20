# sulion

*Quenya **súlë** (breath, spirit, emanation) bound to Aulë, Vala of craftsmanship — "emanation-forge."*

Session broker for Claude Code terminal sessions. Persistent PTYs with structured timeline scrollback, served to any LAN device via web UI.

- Architectural design: [`session-broker-design.md`](session-broker-design.md)
- Invariants and contributor guide: [`CLAUDE.md`](CLAUDE.md)
- E2E coverage plan: [`docs/e2e-coverage-plan.md`](docs/e2e-coverage-plan.md)

## Status

Early development. Backend and frontend MVPs landed; first Komodo deploy (issue #14) is the next verification step.

## Local development

Requires Rust (for the backend), pnpm + Node 24 (for the frontend), and either Docker or an existing Postgres for the backend integration suite.

```bash
make ci              # full lint + unit tests + backend integration tests
make e2e             # Playwright suite against real backend + Postgres + seeded ingest data
make test-rust-integration   # ignored backend integration suite; auto-starts Postgres via Docker if needed

# Backend
cd backend && cargo run                        # SULION_DB_URL must be set

# Backend integration tests against an existing Postgres
SULION_TEST_DB='postgres://postgres:testpass@127.0.0.1:55432/sulion' \
  make test-rust-integration

# Frontend
cd frontend && pnpm install && pnpm dev       # proxies /api and /ws to :8080
```

`make test-rust-integration` runs the ignored backend integration binaries one at a time with
`--test-threads=1`. If `SULION_TEST_DB` is unset, the script starts an ephemeral
`postgres:16` container through Docker, waits for readiness, runs the suite, and cleans it up.

## First-run on TrueNAS

Deploying to TrueNAS uses the standard ahara path: Docker Compose via Komodo, shared TrueNAS Postgres auto-provisioned by the migration Lambda, Komodo stack created on demand by the deploy action.

### 1. ahara-infra registration (one-time, cross-repo)

`project-sulion.tf` under `control/` and a one-line addition to `truenas_db_projects` under `services/db-migrate-truenas.tf`. Already landed — `ahara-infra` commit `3a221d6`.

### 2. Dataset on TrueNAS (one-time, shell entry only)

Create one dataset at `/mnt/apps/apps/sulion` and chown it to the container's dev user:

```bash
zfs create apps/apps/sulion
chown 7321:7321 /mnt/apps/apps/sulion
```

(`7321` is a deliberately unusual uid chosen to avoid colliding with the 1000-series uid that most consumer apps claim. Match this to the `DEV_UID` / `DEV_GID` build args in `backend/Dockerfile` if you need to change it.)

That's the whole bootstrap. The container's entrypoint self-provisions the home-directory subtree on first boot — `~/.claude/`, `~/.ssh/`, `~/.local/bin/`, `~/.config/gh/`, `~/repos/` — and installs the default `.claude/settings.json` with the SessionStart hook pre-wired. No `mkdir -p`, no `truenas-bootstrap.sh`.

### 3. Deploy

Push to `main`. The ahara shared CI workflow builds both images, pushes to GHCR, and the `deploy-truenas` action:

1. Invokes `ahara-db-migrate-truenas` with `project: "sulion"` → creates the `sulion` database, an app role, and publishes `/ahara/truenas-db/sulion/{username,password}` in SSM.
2. Lists Komodo servers, creates the `sulion` stack on-demand (tolerant of already-exists), points it at this repo's `compose.yaml`.
3. Resolves the two SSM paths declared in `secret-paths.yml`, sets them as Komodo stack environment variables, and deploys.

### 4. Drop in your credentials

SSH into TrueNAS and put your personal state directly into the dataset — it appears as `/home/dev/` inside the container:

- SSH keys for `git clone`: `/mnt/apps/apps/sulion/.ssh/` (chmod 0600 for private keys)
- Git identity: `/mnt/apps/apps/sulion/.gitconfig`
- Claude auth: run `claude login` inside a sulion PTY session, or copy an existing `~/.claude/.credentials.json` into `/mnt/apps/apps/sulion/.claude/`
- `gh` token (optional): `/mnt/apps/apps/sulion/.config/gh/hosts.yml`

### 5. Verify

```bash
curl -sf http://192.168.66.3:30080/health
# → {"status":"ok","db":"ok"}
```

Open the UI at `http://192.168.66.3:30080/`, create a repo, spawn a session, run `claude` inside. The SessionStart hook correlates the session; the timeline populates via the ingester polling the bind-mounted `~/.claude/projects/`.

## Ingestion model

All JSONL reads happen in the backend ingester — the REST API and WebSocket paths query Postgres only. See `CLAUDE.md` for the full list of architectural invariants.

## License

MIT — see [`LICENSE`](LICENSE).
