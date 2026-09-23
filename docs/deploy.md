# Deploy

Standard ahara TrueNAS deploy: Docker Compose via Komodo, shared TrueNAS Postgres auto-provisioned by the migration Lambda, Komodo stack created on demand by the deploy action.

The root `compose.yaml` is both the portable service graph and the production
TrueNAS control-plane selection. It starts only frontend, backend/control,
broker, and retrieval. Small role overlays live in
[`deploy/`](../deploy/README.md): the dedicated overlay selects the NixOS
node-side services, while the standalone overlay (plus the TrueNAS policy
overlay on that host) restores the previous combined deployment. Komodo accepts
one Compose path, so
[`deploy/compose.truenas-standalone.yaml`](../deploy/compose.truenas-standalone.yaml)
merges the combined TrueNAS model into one deployable entry point.

The dedicated host configuration, activation boundary, secret paths, and
box-side verification commands live in [`nix/README.md`](../nix/README.md).
Its deployment unit consumes this same Compose graph from an immutable Nix
store path; it never runs the editable checkout under `/home/sulion/repos`.
Fresh hosts are installed through the flake's `bootstrap-enclave` app: Disko
owns partitioning and mounting, while standard `nixos-install` populates the
mounted system. Existing hosts import an administration public-key file with
`install-admin-key` before activating the new SSH configuration.
Those break-glass keys live only in the root-owned
`/var/lib/sulion/config/ssh/authorized_keys`; they are not node credentials,
broker secrets, or source-controlled host configuration.

The common graph now has eight runtime service roles:

- `backend` — main API/control plane; also hosts the loopback runtime only in
  portable standalone mode
- `node` — dedicated PTY management, correlation, repo, worktree, file,
  Git, upload, and direct-Docker runtime; launched devenv containers own shells
  and shadow emulators
- `ingester` — sole node-local Claude/Codex JSONL reader
- `broker` — secret broker, separate container and UID
- `retrieval` — agent-facing transcript/timeline retrieval API
- `code-intel` — agent-facing structural source navigation API
- `runner` — constrained Docker command broker and sole host-socket holder in
  brokered standalone mode; absent from the direct-Docker dedicated role
- `frontend` — static UI + reverse proxy

`node`, `ingester`, code intelligence, and the constrained runner are
profile-gated in the common graph. The dedicated overlay activates the first
three; the standalone overlay activates code intelligence and the runner while
the backend hosts the loopback node runtime. All three core Rust processes
currently use the same workbench image with different entry points; CI still
builds that large artifact once.

Across the production split:

- TrueNAS control uses remote-node mode and has no repo, workspace, transcript,
  home, code-intelligence, runner, or Docker bind;
- the node alone mounts `/home/sulion`, its root-owned identity key, the
  correlation run directory, and the dedicated host's Docker socket;
- the ingester mounts only the two transcript roots read-only; and
- code intelligence reads the local repo/workspace roots on that host.

## One-time cross-repo registration

Sulion needs three cross-repo infra registrations in `ahara-infra`:

- `infrastructure/terraform/control/project-sulion.tf` grants the deployer role enough IAM to create the Sulion Cognito app client, publish SSM parameters, manage the project-owned ALB listener/certificate/DNS, and deploy the Komodo stack.
- `infrastructure/terraform/services/db-migrate-truenas.tf` needs a `sulion` entry in `truenas_db_stacks` with `app` and `broker` database registrations so the shared migration Lambda provisions both databases and publishes `/ahara/truenas-db/sulion/app/{username,password,url}` plus `/ahara/truenas-db/sulion/broker/{username,password,url}`.
- `infrastructure/terraform/network/locals.tf` registers `sulion.services.ahara.io` as an `internal` reverse-proxy upstream at `192.168.66.3:30080`, with buffering disabled and WebSocket upgrades enabled. `internal` means Ahara Infra owns nginx and WireGuard ingress while Sulion Terraform owns the public ALB resources. Port `30081/tcp` is deliberately **not** registered: it is the backend's encrypted LAN-only node endpoint (control channel plus broker/retrieval gateway), and routing it publicly would undo the node pairing boundary.

Sulion also carries project-local Terraform under [`infrastructure/terraform/`](</home/sulion/repos/sulion/infrastructure/terraform>) that creates its `sulion.services.ahara.io` ALB listener rules, ACM certificate, Route53 records, Cognito app client, and publishes:

- `/ahara/cognito/clients/sulion-app`
- `/ahara/auth-trigger/clients/sulion`
- `/ahara/sulion/retrieval-token`
- `/ahara/sulion/code-intel-token`

## One-time TrueNAS bootstrap

The production control plane requires only the broker-state dataset:

```bash
zfs create apps/apps/sulion-broker
chown 7322:7322 /mnt/apps/apps/sulion-broker
```

`apps/apps/sulion-broker` belongs only to the broker container. It holds the
broker master key and is never mounted into control or a node. The split
control plane mounts no development home, repository, or workspace datasets.

UID/GID **7321** is deliberately off the 1000-series consumer range. Pinned in `backend/Dockerfile` via the `DEV_UID` / `DEV_GID` build args; change both together or not at all.

The broker runs as **7322:7322**, configured in [`broker/Dockerfile`](</home/sulion/repos/sulion/broker/Dockerfile>).

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

1. Invokes `ahara-db-migrate-truenas` with `stack_name: "sulion"` → creates every registered Sulion database and publishes `/ahara/truenas-db/sulion/app/{username,password,url}` plus `/ahara/truenas-db/sulion/broker/{username,password,url}` to SSM.
2. Runs `terraform apply` in [`infrastructure/terraform/`](</home/sulion/repos/sulion/infrastructure/terraform>) → creates the Sulion edge listener rules/certificate/DNS and Cognito app client, then publishes `/ahara/cognito/clients/sulion-app` plus `/ahara/auth-trigger/clients/sulion`.
3. Creates (or reuses) the `sulion` Komodo stack pointed at this repo's `compose.yaml`.
4. Resolves the public Cognito identifiers in [`secret-paths.yml`](../secret-paths.yml)
   and deploys. Secret-bearing TrueNAS services enroll with their own workload
   identities and read their database URL and service tokens from SSM at
   startup; the paths are declared beside each service in Compose. The
   dedicated node receives its shared configuration over the signed node
   channel, not from deployment-injected secrets.
5. Advances the `node-release` branch after the shared workflow succeeds. The
   root-owned timer on `sulion-enclave` first activates that commit's NixOS
   generation, then deploys the same commit's node, ingester, and
   code-intelligence images.

No manual Komodo UI setup. No manual SSM puts.

The single TrueNAS topology selector is `truenas_compose_path` in
[`platform.yml`](../platform.yml):

```yaml
# Split deployment: TrueNAS control plane plus sulion-enclave.
truenas_compose_path: compose.yaml

# Combined host operation: all development services return to TrueNAS.
truenas_compose_path: deploy/compose.truenas-standalone.yaml
```

Changing that one value and pushing `main` is sufficient to switch the Komodo
stack between the two supported TrueNAS roles. CI renders both paths on every
change. The combined role expects its development home at
`/mnt/apps/apps/sulion`, with `repos` and `workspaces` mounted explicitly
because a parent bind does not cross nested ZFS dataset mount points.

Deploy `ahara-infra` before the first Sulion edge deployment so the internal
nginx upstream, WireGuard ingress, and Sulion deployer permissions already
exist. The production TrueNAS deploy replaces only control-plane services: a
backend replacement drops browser attachments, while devenv-owned PTYs continue
and reconnect. Switching to the combined role terminates PTYs owned by its
combined backend. A node release also leaves shells running: PTY masters live
in the devenv container (`sulion-devenv`, launched and adopted by the node,
deliberately not a compose service), which keeps serving its shells while the
node is recreated and redials it after.

The backend/control container owns the main `sulion` database migrations and
Postgres-only startup repair. Node, ingester, retrieval, and code intelligence
do not run the shared SQLx migrations; they wait in-app for the
backend-applied migration set before starting their loops.

## PTY and toolset releases

`devenv` is a toolset-only image listed under `content_addressed_images` in
`platform.yml`. Unchanged `devenv/` content reuses the image; backend-only
releases do not replace the containers holding existing shells. The node keys
containers by resolved image ID and starts new sessions on the current image.

The node delivers `sulion-devenv` to a versioned path on `/run/sulion` when
creating a container. It updates the separate `/run/sulion/bin/sulion` CLI
symlink on every node start, including when it adopts an existing container.
Running shells therefore pick up new CLI commands on their next invocation.

Use **Upgrade toolset (restarts shell)** on one live session to move it to the
current image. It starts a default shell with the same session ID and workspace;
resume the agent separately. Non-current containers are removed when stopped
or empty. Host reboot and combined-role replacement still end their shells.
See [architecture](architecture.md#pty-lifetime-and-toolset-upgrades).

Claude Code is seeded from the image into its native install under the
persistent home on first start. Its own updater can subsequently update that
copy without sudo, and the copy survives image changes. This is distinct from
the explicit shell/toolset upgrade above.

## Retrieval Search

The retrieval service reads the existing Sulion Postgres tables directly. Migrations add non-blocking indexes for lexical search and a `retrieval_embeddings` table that stores embedding vectors plus source keys only; transcript text remains in the canonical event/timeline tables.

Lexical search uses `pg_trgm`, which is installed by migration as `CREATE EXTENSION IF NOT EXISTS pg_trgm`.

Semantic search requires `pgvector`. Migration `0090` runs `CREATE EXTENSION IF NOT EXISTS vector`, owns the `embedding_vector vector(768)` column, and `0091` the HNSW index; the extension must be available on the Postgres host (the TrueNAS image ships it, and it was installed there before this migration existed, so the migration is a no-op apart from dropping the old `REAL[]` column). The retrieval service verifies the column and index at startup and refuses to serve if either is missing; there is no exact-scan fallback. Semantic indexing schedules durable cursor backfills in `retrieval_embedding_backfills`, records source freshness in `retrieval_embedding_sources`, and drains pending sources through the local embedding service configured by `SULION_RETRIEVAL_EMBEDDING_URL`, defaulting to `http://192.168.66.3:5361` with `nomic-ai/nomic-embed-text-v1.5`. On an empty semantic source state, startup schedules the initial backfills automatically; the worker runs continuously while backlog exists and uses `SULION_RETRIEVAL_INDEX_SECONDS` only as the idle interval.

The PTY helper is `sulion-retrieve`; the full API contract is in [`docs/retrieval.md`](retrieval.md).

## Transcript archive

The control process runs the archive loop (`backend/src/archive/`) when
`SULION_ARCHIVE_BUCKET` is set. The deploy resolves that name from
`/ahara/sulion/archive-bucket`, which this repository's Terraform publishes
alongside the bucket itself (`infrastructure/terraform/archive.tf`:
`sulion-archive-<account>`, versioned, SSE-S3, public access blocked,
Glacier Instant Retrieval after 30 days, current objects never expire). The
backend's machine role gains put, get, and list on that one bucket, inside
the `sulion-*` namespace the shared workload permissions boundary already
allows. The deployer creates and configures the bucket through the
`s3-private-storage` policy module declared for this project in
`ahara-infra` (`project-sulion.tf`); that module carries no bucket-policy
calls, so the bucket has no TLS-only policy and transport security rests on
the clients, which use HTTPS. The loop talks to S3 through the AWS SDK with
one reused client, authenticated by the profile the Roles Anywhere
bootstrap writes; it never runs the `aws` CLI, whose PTY wrapper on the
image's PATH routes through the secret broker with a PTY grant the control
process does not have, and whose per-call start-up made the first export
take hours.

Every `SULION_ARCHIVE_INTERVAL_DAYS` (30) the loop:

1. dumps the durable tables with `pg_dump -Fc` to `db/` — the pgdg
   PostgreSQL 18 client at `SULION_PG_DUMP=/usr/pgsql-18/bin/pg_dump`, since
   the TrueNAS server is 18 and the image's appstream client is 16;
2. exports every session idle for `SULION_ARCHIVE_MIN_IDLE_DAYS` (30) to
   `sessions/<agent>/<yyyy>/<mm>/<session>.jsonl.zst` and verifies it by
   `HEAD`;
3. purges sessions exported `SULION_ARCHIVE_PURGE_AFTER_DAYS` (0: in the
   same cycle, once the upload's hash is confirmed) ago to their turn
   digest, rolling cost and file churn up first;
4. prunes finished backfill and job rows by age.

Progress is an `ingest_jobs` row in the Jobs panel. `sulion archive status`
in a PTY, or `GET /api/admin/archive`, shows the store, whether purging is
enabled, the last cycle and dump, counts, and recent requests. An empty
`SULION_ARCHIVE_BUCKET` disables the loop; `SULION_ARCHIVE_DIR` points it at
a directory instead (tests, or a local stand-in).

### Purging is a committed setting

`SULION_ARCHIVE_PURGE_ENABLED` in `compose.yaml` is a literal. With `"0"` a
cycle exports every idle session and writes the durable dump but purges
nothing, however old the exports are; with `"1"` it purges. Changing it is a
commit, reviewed and deployed like any other change; there is no command or
API that flips it. The loop records the value it started with in
`archive_state`, which is what `sulion archive status` shows.

The first deployment ran with `"0"`, and the line was changed to `"1"` only
after the first export had been checked:

```bash
sulion archive run              # or wait for the scheduled cycle
sulion archive list             # the run request completes with counts
sulion archive verify --deep    # downloads every object and re-hashes it
sulion archive list             # verify reports ok / missing / mismatched
```

`verify` without `--deep` checks existence and the stored hash and counts
only. While purging is disabled, `restore --all` and `--purge-after` restore
but do not purge again, and say so in the request result. Changing the line
back to `"0"` disables deletion again on the next deploy. A new deployment
of Sulion should start the same way: `"0"`, one cycle, `verify --deep`,
then the commit to `"1"`.

### Restore a session or the whole history

```bash
sulion archive restore --session <agent-session-uuid>
sulion archive restore --month 2026-05
sulion archive restore --repo sulion --purge-after
sulion archive restore --all            # oldest month first, purges again after each
sulion archive list
```

A restore replays the archived lines through the normal ingest path; the
session then looks exactly as it did before the purge, and `--purge-after`
returns it to the digest once the replay is verified. `--all --purge-after`
is a full re-index from the archive under current parsing rules and never
holds more than one restored session at a time.

### Restore the durable dump

Only for rebuilding the instance. The dump holds plans, sessions, settings,
identities, rollups, and per-session skeletons; transcript content is in the
session objects.

```bash
aws s3 cp s3://sulion-archive-<account>/db/sulion-durable-<date>.dump .
/usr/pgsql-18/bin/pg_restore --dbname "$SULION_DB_URL" --no-owner --no-privileges \
  --clean --if-exists sulion-durable-<date>.dump
```

Then start the control process (it applies any newer migrations) and run
`sulion archive restore --all` for whatever history is wanted back in full.

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

## Standalone TrueNAS credentials

The following paths apply only to the combined TrueNAS role. Its dataset root
is the container's `/home/sulion/`:

- SSH keys: `/mnt/apps/apps/sulion/.ssh/` (private keys chmod 0600)
- Git identity: `/mnt/apps/apps/sulion/.gitconfig`
- Claude auth: `claude login` inside a sulion PTY, or copy an existing `~/.claude/.credentials.json` into `/mnt/apps/apps/sulion/.claude/`
- Optional `gh` token: `/mnt/apps/apps/sulion/.config/gh/hosts.yml`

Secrets are no longer intended to live in repo-local `.env` files. The broker stores encrypted secret payloads in the separate `sulion_broker` database, with the master key remaining only on `/mnt/apps/apps/sulion-broker/master.key`.

## Verify

```bash
curl -sf http://192.168.66.3:30080/health
# → {"status":"ok","db":"ok","role":"control-plane","development_node":"connected"}

curl -sf https://sulion.services.ahara.io/health
# → {"status":"ok","db":"ok","role":"control-plane","development_node":"connected"}
```

`development_node` is `unavailable` while the node is disconnected; that does
not make the control health check fail. UI and Postgres-backed history remain
available, while filesystem and PTY mutations return `503`.

UI is at `https://sulion.services.ahara.io/`. The frontend blocks on Cognito
sign-in. Browser REST and broker-management requests carry the Cognito token;
PTY WebSockets use a short-lived, one-use ticket minted by an authenticated
request. All node traffic — control channel, broker, retrieval — uses the
single encrypted endpoint at `192.168.66.3:30081` (`wss://…/ws/nodes`,
`https://…/broker`, `https://…/retrieval`), TLS-terminated in the control
process with a certificate the node pins. No node traffic reaches the public
hostname or crosses the LAN in the clear.

## Networking

The public path is shared Ahara ALB/WAF → EC2 nginx → WireGuard → the frontend
published on `192.168.66.3:30080`. The direct LAN URL remains available for
operations and rollback. Development-node pairing does not use that path at
all: the frontend returns 404 for `/ws/nodes`, and nodes instead reach the
backend directly on `192.168.66.3:30081`, which no upstream registration points
at. Because that hop has no proxy in it, the backend sees each node's real
address and enforces the node LAN on it. Development ports `26000-26010` are published by
workloads on `sulion-enclave`, not by the TrueNAS backend. A process in a
Sulion PTY must bind `0.0.0.0` on one of those ports to be reachable from the
LAN, for example:

```bash
npm run dev -- --host 0.0.0.0 --port 26000
```

Those dev ports are direct LAN exposure and are not routed through Sulion auth.

The standalone stack also creates the internal Docker network `sulion`;
runner-launched containers join that network automatically. Public listener
rules apply ALB JWT validation to browser routes. Node pairing approval is a
browser-authenticated action. The node WebSocket, signed broker redemption/PTY
registration, and bearer-authenticated retrieval are
application-authenticated machine routes.
