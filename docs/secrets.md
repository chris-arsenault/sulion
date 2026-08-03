# Secrets

Sulion supports exactly two credential-consumption paths:

- `with-cred` for general env-bundle injection
- `aws` as a wrapper over the real AWS CLI

Nothing else is part of the product contract. There is no general shell-wide secret export, no ad hoc tool wrappers, and no alternate brokered execution path.

## Purpose

The secrets system exists to let the UI manage credentials and PTY-scoped grants without putting raw secret material into repo files, shell startup files, or the main Sulion database.

The boundary is:

- the **frontend** manages secret setup and grant actions through the broker
- the **development node** launches PTYs and ships the wrapper tools
- the **broker** stores encrypted secret bundles and redeems active grants

The host's LAN-only SSH administration keys are outside this product secret
flow. They authorize break-glass NixOS maintenance, live in a root-owned runtime
file on `sulion-enclave`, and are never exposed to PTYs or used for node
authentication.

Terraform creates the metadata-only AWS Secrets Manager entry
`sulion-enclave-admin-ssh-key` as an operator recovery backup for the matching
private key. Terraform deliberately does not manage a secret version, so key
material never enters Terraform configuration or state. The resource also
blocks Terraform destruction and retains AWS's 30-day deletion recovery
window. Populate and rotate the value manually from the administration
workstation that owns the key; the entry is not granted to Sulion services,
the broker, or PTYs.

## Shape

Three components participate:

- **Frontend**
  - calls `/broker/*` directly
  - uses the user's Cognito JWT for secret management and grant changes
  - exposes the Secrets tab in the main work area
- **Backend**
  - does not store the broker master key
  - does not unlock secrets through alternate routes
  - registers a per-PTY public key with the broker
  - launches PTYs with broker URL, PTY id, and the private key path in the environment
- **Broker**
  - separate service and container
  - stores encrypted secret payloads in the `sulion_broker` database
  - decrypts them with a master key mounted only into the broker container
  - verifies signed use requests from PTY wrappers
  - enforces grants for wrapper use

## Data model

A secret is an env bundle: one secret id maps to one set of environment variables.

Examples:

- `claude-api`
  - `ANTHROPIC_API_KEY=sxxx`
- `openai-api`
  - `OPENAI_API_KEY=sk-...`
- `AWS`
  - `AWS_ACCESS_KEY_ID=...`
  - `AWS_SECRET_ACCESS_KEY=...`
  - `AWS_SESSION_TOKEN=...`
  - `AWS_REGION=...`

Each secret also carries metadata:

- `id`
- `description`
- `scope`
- optional `repo`
- derived `env_keys`

The broker stores the env map encrypted at rest. The UI lists metadata and env key names; it is not intended to act as a raw secret dump after creation.

## Grant model

Grants are scoped to:

- `pty_session_id`
- `secret_id`
- `expires_at`

That means a PTY can have one or more env bundles enabled, and the same grant
can be redeemed through either supported wrapper. The wrapper name is
audit/runtime context, not part of the grant relationship.

### What the grant scope does and does not separate

The scope is a boundary between **locked and unlocked**, not between concurrent
terminals. A secret nobody has unlocked is not reachable from any PTY, and a
grant that expires or is revoked stops being redeemable. That is the guarantee.

It is not an isolation boundary between one terminal and another. Every PTY runs
as the same identity (`sulion`, uid 7321), so the per-PTY key files under
`/run/sulion/pty-keys/` are readable by every PTY regardless of their `0600`
mode, and an agent can sign broker requests as a different `pty_session_id`.
Redeemed values also land in the spawned process's environment, which any
process of the same uid can read from `/proc/<pid>/environ` for the lifetime of
that command.

The practical consequence: **anything unlocked in one terminal should be treated
as reachable by an agent in another.** Grant scoping limits what is live at a
given moment and records who asked for it; it does not contain a hostile or
prompt-injected agent to its own terminal. Per-terminal containment would
require per-PTY uids or handing the key to the PTY as an inherited descriptor
rather than a readable path.

Grants are created and revoked from terminal/session context menus. The Secrets tab is only for creating, updating, and deleting secret bundles.

## Runtime use

Two wrapper tools are on the PTY `PATH`.

### `with-cred`

General-purpose env injection for one command:

```sh
with-cred claude-api -- claude
with-cred openai-api -- codex
with-cred -- make test
```

Rules:

- `with-cred <secret-id> -- <command...>` injects one specific env bundle
- `with-cred -- <command...>` injects every currently enabled bundle for that PTY
- `with-cred` uses the PTY's active credential grants, regardless of the target command name

### `aws`

The PTY image ships an `aws` wrapper at `/opt/sulion/bin/aws`. It redeems the active PTY grant for any enabled secret bundle that contains both `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, then execs the real AWS CLI.

From the user or agent perspective:

```sh
aws s3 ls
aws sts get-caller-identity
```

works normally when the PTY has an active grant for an AWS-shaped secret and fails cleanly when it does not.

## Conflict handling

`with-cred -- <command...>` may combine multiple unlocked env bundles. If two active bundles define the same environment variable name, the broker rejects the request instead of silently choosing one value.

This is intentional. Secret merges must be explicit, not order-dependent.

## Runtime wiring

PTYs need these runtime values:

- `SULION_PTY_ID`
- `SULION_SECRET_BROKER_URL`
- `SULION_SECRET_BROKER_KEY_PATH`

The node injects them when it launches the PTY. Wrapper use signs each broker
request with the PTY private key. The broker verifies that signature against
the public key registered for that PTY before checking active grants.

The signature authenticates *a* PTY on this node, not specifically the calling
one: both the claimed id and the key path arrive as ordinary environment
variables, and under the shared uid every PTY can read every key. Treat it as
proof that the request came from the node, plus a correct-by-default attribution
for audit — not as proof of which terminal made it. See
[the grant scope note](#what-the-grant-scope-does-and-does-not-separate).

The backend-to-broker registration token is generated by Sulion Terraform and published to SSM at:

- `/ahara/sulion/secret-broker-registration-token`

The TrueNAS broker consumes that value through deployment secret injection.
The dedicated node is not provisioned with it: the control plane forwards it,
along with the database credentials and retrieval token, over the authenticated
node channel once an operator approves the node's identity key. The node writes
them to root-owned host state that the PTY identity cannot read. See
[node-protocol.md](node-protocol.md).

Only a fixed key list crosses that boundary. The broker master key and Cognito
credentials are not in it and stay on TrueNAS, so a node — or anything that
compromises one — never sees them. The node reaches the broker's
machine-authenticated routes over the encrypted node endpoint at
`https://192.168.66.3:30081/broker`, never the public hostname: no node
traffic leaves the network or crosses the LAN in the clear. It does not
connect to a local broker. The token is not forwarded into PTY shells. The control process has no
PTY credential file, and neither control nor the development node receives the
broker master key.

The node/code-intelligence token is the exception that is *not* forwarded: both
ends live on the enclave's loopback, so that host generates its own and
`/ahara/sulion/code-intel-token` applies only to the control plane.

## UI surface

Secret setup lives in a dedicated **Secrets** tab in the main content area.

It supports:

- creating and editing env-bundle secrets
- setting metadata such as id, description, scope, and repo
- adding explicit key/value pairs
- overwriting an existing env value without reading the old value

Existing secret values are not returned by browser read endpoints. Editing an existing bundle shows only the env key names. Leaving an existing value blank preserves it; entering a new value overwrites it.

Grants are managed from terminal/session context menus:

- right-click a terminal or session
- open **Secrets**
- use **Enable secret** to choose a secret and TTL
- use **Active secrets** to see remaining TTL and click a grant to revoke it immediately

## Broker API

Authenticated browser endpoints:

- `GET /broker/v1/secrets`
- `GET /broker/v1/secrets/:id`
- `PUT /broker/v1/secrets/:id`
- `DELETE /broker/v1/secrets/:id`
- `GET /broker/v1/grants?pty_session_id=<uuid>`
- `POST /broker/v1/grants`
- `DELETE /broker/v1/grants`

Authenticated PTY-use endpoint:

- `POST /broker/v1/use`

`/broker/v1/use` accepts signed PTY requests only. It cannot create, extend, or mutate unlock state.

Backend registration endpoints:

- `POST /broker/v1/pty-credentials`
- `DELETE /broker/v1/pty-credentials/:id`

These are authenticated with the backend registration token and are only for registering or revoking PTY public keys.

## Non-goals

This system does not support:

- shell-global secret export
- direct `.env` file management
- arbitrary wrapper generation for random tools
- using broker credentials directly from the PTY
- storing the broker master key in the control, node, or PTY container
