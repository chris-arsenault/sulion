# Deploy

Standard ahara TrueNAS deploy: Docker Compose via Komodo, shared TrueNAS Postgres auto-provisioned by the migration Lambda, Komodo stack created on demand by the deploy action.

Sulion now has four services:

- `backend` — main API + PTY runtime
- `broker` — secret broker, separate container and UID
- `runner` — constrained Docker command broker, only service with the host Docker socket
- `frontend` — static UI + reverse proxy

## One-time cross-repo registration

Sulion now needs two cross-repo infra registrations:

- `infrastructure/terraform/control/project-sulion.tf` grants the deployer role enough IAM to create the Sulion Cognito app client, publish SSM parameters, and deploy the Komodo stack.
- `infrastructure/terraform/services/db-migrate-truenas.tf` needs a `sulion` entry in `truenas_db_stacks` with `app` and `broker` database registrations so the shared migration Lambda provisions both databases and publishes `/ahara/truenas-db/sulion/app/{username,password}` plus `/ahara/truenas-db/sulion/broker/{username,password}`.

Sulion also carries project-local Terraform under [`infrastructure/terraform/`](</home/dev/repos/sulion/infrastructure/terraform>) that creates the Cognito app client and publishes:

- `/ahara/cognito/clients/sulion-app`
- `/ahara/auth-trigger/clients/sulion`

## One-time TrueNAS bootstrap

Four datasets, each chowned to the matching container user:

```bash
zfs create apps/apps/sulion
chown 7321:7321 /mnt/apps/apps/sulion

zfs create apps/apps/sulion/repos
chown 7321:7321 /mnt/apps/apps/sulion/repos

zfs create apps/apps/sulion/workspaces
chown 7321:7321 /mnt/apps/apps/sulion/workspaces

zfs create apps/apps/sulion-broker
chown 7322:7322 /mnt/apps/apps/sulion-broker
```

Why four:

- `apps/apps/sulion` is the dev user's home. Credentials, shell history, claude sessions, etc.
- `apps/apps/sulion/repos` holds the working trees. On its own dataset so you can expose it via NFS/SMB, snapshot it on a different cadence, and mount it from other machines without carrying home-dir state.
- `apps/apps/sulion/workspaces` holds Sulion-created Git worktrees for isolated sessions.
- `apps/apps/sulion-broker` belongs only to the broker container. It holds the broker master key and is **never** mounted into the PTY container.

ZFS snapshots don't recurse into child datasets by default, so the nested layout keeps parent-level snapshots light — pass `-r` to `zfs snapshot` when you explicitly want everything.

UID/GID **7321** is deliberately off the 1000-series consumer range. Pinned in `backend/Dockerfile` via the `DEV_UID` / `DEV_GID` build args; change both together or not at all.

The broker runs as **7322:7322**, configured in [`broker/Dockerfile`](</home/dev/repos/sulion/broker/Dockerfile>).

`compose.yaml` bind-mounts each dataset explicitly — Docker's plain bind doesn't follow nested ZFS datasets under the parent, so every dataset needs its own entry. This also means you can add a `zfs create apps/apps/sulion/<something>` later and the compose file keeps working until you're ready to wire it in.

That's the whole bootstrap. `backend/entrypoint.sh` self-provisions `~/.claude/`, `~/.ssh/`, `~/.local/bin/`, `~/.config/gh/`, `~/repos/`, and `~/workspaces/` on first boot and pre-writes `.claude/settings.json` wiring the `SessionStart` hook.

## Broker key

Generate a 32-byte raw master key file on the host:

```bash
dd if=/dev/urandom of=/mnt/apps/apps/sulion-broker/master.key bs=32 count=1
chmod 0400 /mnt/apps/apps/sulion-broker/master.key
chown 7322:7322 /mnt/apps/apps/sulion-broker/master.key
```

The broker container mounts this dataset read-only at `/var/lib/sulion-broker`. The backend/PTY container must never see this file or dataset.

## Deploy

Push to `main`. The shared ahara CI workflow builds all Sulion images, pushes to GHCR, and the `deploy-truenas` action:

1. Invokes `ahara-db-migrate-truenas` with `stack_name: "sulion"` → creates every registered Sulion database and publishes `/ahara/truenas-db/sulion/app/{username,password}` plus `/ahara/truenas-db/sulion/broker/{username,password}` to SSM.
2. Runs `terraform apply` in [`infrastructure/terraform/`](</home/dev/repos/sulion/infrastructure/terraform>) → creates the Sulion Cognito app client and publishes `/ahara/cognito/clients/sulion-app` plus `/ahara/auth-trigger/clients/sulion`.
3. Creates (or reuses) the `sulion` Komodo stack pointed at this repo's `compose.yaml`.
4. Resolves the SSM paths declared in [`secret-paths.yml`](</home/dev/repos/sulion/secret-paths.yml>), sets them as Komodo stack env vars, and deploys.

No manual Komodo UI setup. No manual SSM puts.

## Drop in credentials

SSH into TrueNAS. The dataset root is the container's `/home/dev/`:

- SSH keys: `/mnt/apps/apps/sulion/.ssh/` (private keys chmod 0600)
- Git identity: `/mnt/apps/apps/sulion/.gitconfig`
- Claude auth: `claude login` inside a sulion PTY, or copy an existing `~/.claude/.credentials.json` into `/mnt/apps/apps/sulion/.claude/`
- Optional `gh` token: `/mnt/apps/apps/sulion/.config/gh/hosts.yml`

Secrets are no longer intended to live in repo-local `.env` files. The broker stores encrypted secret payloads in the separate `sulion_broker` database, with the master key remaining only on `/mnt/apps/apps/sulion-broker/master.key`.

## Verify

```bash
curl -sf http://192.168.66.3:30080/health
# → {"status":"ok","db":"ok"}
```

UI at `http://192.168.66.3:30080/`. The frontend now blocks on Cognito sign-in and the backend requires a valid Sulion app token on API and websocket routes. After login, create a repo, spawn a session, run `claude`. The `SessionStart` hook correlates the agent session; the timeline populates from ingested JSONL.

## Networking

LAN-only via WireGuard, published on `192.168.66.3:30080`. No reverse-proxy route. When public exposure is wanted, add a `reverse_proxy_routes` entry in `ahara-network` with `auth = "jwt-validation"` — the code does not assume the ALB path anywhere.
