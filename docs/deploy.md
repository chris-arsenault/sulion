# Deploy

Standard ahara TrueNAS deploy: Docker Compose via Komodo, shared TrueNAS Postgres auto-provisioned by the migration Lambda, Komodo stack created on demand by the deploy action.

## One-time cross-repo registration

Landed as `ahara-infra` commit `3a221d6`:

- `project-sulion.tf` under `control/`
- One-line `truenas_db_projects` entry under `services/db-migrate-truenas.tf`

Nothing to redo per-environment.

## One-time TrueNAS bootstrap

One dataset, chowned to the container's `dev` user:

```bash
zfs create apps/apps/sulion
chown 7321:7321 /mnt/apps/apps/sulion
```

UID/GID **7321** is deliberately off the 1000-series consumer range. Pinned in `backend/Dockerfile` via the `DEV_UID` / `DEV_GID` build args; change both together or not at all.

That's the whole bootstrap. `backend/entrypoint.sh` self-provisions `~/.claude/`, `~/.ssh/`, `~/.local/bin/`, `~/.config/gh/`, `~/repos/` on first boot and pre-writes `.claude/settings.json` wiring the `SessionStart` hook.

## Deploy

Push to `main`. The shared ahara CI workflow builds both images, pushes to GHCR, and the `deploy-truenas` action:

1. Invokes `ahara-db-migrate-truenas` with `project: "sulion"` → creates the database, app role, and publishes `/ahara/truenas-db/sulion/{username,password}` to SSM.
2. Creates (or reuses) the `sulion` Komodo stack pointed at this repo's `compose.yaml`.
3. Resolves the SSM paths declared in `secret-paths.yml`, sets them as Komodo stack env vars, and deploys.

No manual Komodo UI setup. No manual SSM puts.

## Drop in credentials

SSH into TrueNAS. The dataset root is the container's `/home/dev/`:

- SSH keys: `/mnt/apps/apps/sulion/.ssh/` (private keys chmod 0600)
- Git identity: `/mnt/apps/apps/sulion/.gitconfig`
- Claude auth: `claude login` inside a sulion PTY, or copy an existing `~/.claude/.credentials.json` into `/mnt/apps/apps/sulion/.claude/`
- Optional `gh` token: `/mnt/apps/apps/sulion/.config/gh/hosts.yml`

## Verify

```bash
curl -sf http://192.168.66.3:30080/health
# → {"status":"ok","db":"ok"}
```

UI at `http://192.168.66.3:30080/`. Create a repo, spawn a session, run `claude`. The `SessionStart` hook correlates the agent session; the timeline populates from ingested JSONL.

## Networking

LAN-only via WireGuard, published on `192.168.66.3:30080`. No reverse-proxy route. When public exposure is wanted, add a `reverse_proxy_routes` entry in `ahara-network` with `auth = "jwt-validation"` — the code does not assume the ALB path anywhere.
