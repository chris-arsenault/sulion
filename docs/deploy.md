# Deploy

Standard ahara TrueNAS deploy: Docker Compose via Komodo, shared TrueNAS Postgres auto-provisioned by the migration Lambda, Komodo stack created on demand by the deploy action.

Sulion now has six services:

- `backend` — main API + PTY runtime
- `broker` — secret broker, separate container and UID
- `retrieval` — agent-facing transcript/timeline retrieval API
- `code-intel` — agent-facing structural source navigation API
- `runner` — constrained Docker command broker, only service with the host Docker socket
- `frontend` — static UI + reverse proxy

## One-time cross-repo registration

Sulion needs three cross-repo infra registrations in `ahara-infra`:

- `infrastructure/terraform/control/project-sulion.tf` grants the deployer role enough IAM to create the Sulion Cognito app client, publish SSM parameters, manage the project-owned ALB listener/certificate/DNS, and deploy the Komodo stack.
- `infrastructure/terraform/services/db-migrate-truenas.tf` needs a `sulion` entry in `truenas_db_stacks` with `app` and `broker` database registrations so the shared migration Lambda provisions both databases and publishes `/ahara/truenas-db/sulion/app/{username,password}` plus `/ahara/truenas-db/sulion/broker/{username,password}`.
- `infrastructure/terraform/network/locals.tf` registers `sulion.services.ahara.io` as an `internal` reverse-proxy upstream at `192.168.66.3:30080`, with buffering disabled and WebSocket upgrades enabled. `internal` means Ahara Infra owns nginx and WireGuard ingress while Sulion Terraform owns the public ALB resources.

Sulion also carries project-local Terraform under [`infrastructure/terraform/`](</home/dev/repos/sulion/infrastructure/terraform>) that creates its `sulion.services.ahara.io` ALB listener rules, ACM certificate, Route53 records, Cognito app client, and publishes:

- `/ahara/cognito/clients/sulion-app`
- `/ahara/auth-trigger/clients/sulion`
- `/ahara/sulion/retrieval-token`
- `/ahara/sulion/code-intel-token`

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
2. Runs `terraform apply` in [`infrastructure/terraform/`](</home/dev/repos/sulion/infrastructure/terraform>) → creates the Sulion edge listener rules/certificate/DNS and Cognito app client, then publishes `/ahara/cognito/clients/sulion-app` plus `/ahara/auth-trigger/clients/sulion`.
3. Creates (or reuses) the `sulion` Komodo stack pointed at this repo's `compose.yaml`.
4. Resolves the SSM paths declared in [`secret-paths.yml`](</home/dev/repos/sulion/secret-paths.yml>), sets them as Komodo stack env vars, and deploys.

No manual Komodo UI setup. No manual SSM puts.

Deploy `ahara-infra` before the first Sulion edge deployment so the internal
nginx upstream, WireGuard ingress, and Sulion deployer permissions already
exist. Run the Sulion deployment from outside a Sulion PTY: replacing the
backend container terminates every active shell.

The backend container owns the main `sulion` database migrations. Retrieval and
code-intelligence do not run the shared SQLx migrations; they wait in-app for
the backend-applied migration set before starting their API/background loops.

## Retrieval Search

The retrieval service reads the existing Sulion Postgres tables directly. Migrations add non-blocking indexes for lexical search and a `retrieval_embeddings` table that stores embedding vectors plus source keys only; transcript text remains in the canonical event/timeline tables.

Lexical search uses `pg_trgm`, which is installed by migration as `CREATE EXTENSION IF NOT EXISTS pg_trgm`.

Semantic search works in two tiers:

- Without `pgvector`, embeddings are stored in `REAL[]` and semantic search can exact-scan that table.
- For indexed ANN search, a database superuser must run `CREATE EXTENSION vector;` once in the `sulion` database. The extension must already be available on the Postgres host. The current TrueNAS Postgres image has it available, so this is an install step, not a recompilation step.

After `vector` is installed, the retrieval service idempotently adds the `embedding_vector vector(768)` column and HNSW index on startup. Semantic indexing schedules durable cursor backfills in `retrieval_embedding_backfills`, records source freshness in `retrieval_embedding_sources`, and drains pending sources through the local embedding service configured by `SULION_RETRIEVAL_EMBEDDING_URL`, defaulting to `http://192.168.66.3:5361` with `nomic-ai/nomic-embed-text-v1.5`. On an empty semantic source state, startup schedules the initial backfills automatically; the worker runs continuously while backlog exists and uses `SULION_RETRIEVAL_INDEX_SECONDS` only as the idle interval.

The PTY helper is `sulion-retrieve`; the full API contract is in [`docs/retrieval.md`](retrieval.md).

## Code Intelligence

The code-intelligence service indexes compact structural facts for mounted repos
and workspaces. It stores roots, file freshness, symbols, references, imports,
and index jobs in Postgres. It does not store full source text or serialized AST
blobs. Source text is read from the read-only repo/workspace mounts at query
time.

Index refresh is dirty marking, not foreground indexing: startup performs one
discovery pass for fresh deployments, `sulion-code refresh` marks discovered
files pending and records deleted files, and the background worker incrementally
drains pending rows and writes symbols/references.

The service uses Tree-sitter for syntactic parsing, ast-grep for structural
search and diff-only patch generation, and persistent language servers for
semantic `def`/`refs` resolution for recently active roots. Rust uses one
rust-analyzer per active root; TypeScript, TSX, JavaScript, and JSX share one
TypeScript-family server per active root. Servers start lazily, expire after
`SULION_CODE_INTEL_LSP_IDLE_SECONDS` (default 1200), and are bounded by
`SULION_CODE_INTEL_LSP_MAX_SERVERS` (default 6). The code-intel image includes
Node, typescript-language-server, and a Rust toolchain with rust-analyzer so
Rust semantic navigation can load real cargo workspaces. Rust analyzer writes
build artifacts to the service cache through `CARGO_TARGET_DIR`; repo and
workspace mounts remain read-only. Fallback and language-server health are
visible through `sulion-code status`.

The PTY helper is `sulion-code`; the full command contract is in
[`docs/code-intel.md`](code-intel.md), and the durable design decision is in
[`docs/adrs/0001-code-intelligence-agent-tool.md`](adrs/0001-code-intelligence-agent-tool.md).

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

curl -sf https://sulion.services.ahara.io/health
# → {"status":"ok","db":"ok"}
```

UI at `https://sulion.services.ahara.io/`. The frontend blocks on Cognito sign-in. REST and broker requests carry the Cognito token; PTY WebSockets use a short-lived, one-use ticket minted by an authenticated request. After login, create a repo, spawn a session, run `claude`. The `SessionStart` hook correlates the agent session; the timeline populates from ingested JSONL.

## Networking

The public path is shared Ahara ALB/WAF → EC2 nginx → WireGuard → the frontend published on `192.168.66.3:30080`. The direct LAN URL remains available for operations and rollback. The backend container also publishes PTY dev-server slots on `192.168.66.3:26000-26010`. A process in a Sulion PTY must bind `0.0.0.0` on one of those ports to be reachable from the LAN, for example:

```bash
npm run dev -- --host 0.0.0.0 --port 26000
```

Those dev ports are direct LAN exposure and are not routed through Sulion auth.

The stack also creates the internal Docker network `sulion`; runner-launched containers join that network automatically so PTY workflows can reach them by container name. Public listener rules apply ALB JWT validation to Cognito-protected HTTP routes, while the application remains authoritative for public pairing, device-token, and one-use WebSocket-ticket routes.
