# Secrets

Sulion supports exactly one credential-consumption path: `with-cred --
<command...>`, which injects every secret granted to the terminal into that
command's environment.

Nothing else is part of the product contract. There is no general shell-wide
secret export, no tool-specific wrapper, no redemption of a named secret, and
no alternate brokered execution path.

## Purpose

The secrets system lets the UI manage credentials, timed terminal grants, and
permanent repository grants without putting raw secret material into repo
files, shell startup files, or the main Sulion database.

The boundary is:

- the **frontend** manages secret setup and grant actions through the broker
- the **development node** launches PTYs and ships `with-cred`
- the **broker** stores encrypted secret bundles and redeems active grants

The host's SSH administration keys are outside this product secret flow. The
public keys live in a root-owned runtime file on `sulion-enclave`; the private
key used for administration lives as `devbox-ssh-key` in the trust appliance's
secret store. It is never exposed to Sulion PTYs or used for node
authentication, and the host firewall admits it only from that appliance.

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
  - verifies signed use requests from `with-cred`
  - enforces grants on redemption

## Data model

A secret is an env bundle: one secret id maps to one set of environment
variables, with any names and values. The id identifies the bundle in the UI
and in grants; nothing redeems a secret by id.

Each secret also carries metadata:

- `id`
- `description`
- `scope`
- optional `repo`
- derived `env_keys`

The broker stores the env map encrypted at rest. The UI lists metadata and env key names; it is not intended to act as a raw secret dump after creation.

## Grant model

Timed terminal grants are scoped to:

- `pty_session_id`
- `secret_id`
- `expires_at`

That means a PTY can have one or more env bundles enabled; `with-cred --`
redeems all of them together.

Permanent repository grants are scoped to `repo` and `secret_id`, with no
expiry. They apply to existing and future PTYs registered for that repository
until explicitly revoked. The node registers the session's repository with
its public key and refreshes that registration when adopting a surviving
shell. Wrappers cannot choose a repository through their request or cwd.
Sessions without a registered repository cannot create or redeem repository
grants; timed terminal grants still work. Collection sessions use their primary
repository, not every collection member. Grants use the repository name;
renaming a repository does not transfer its permanent grants.

A secret may have both a timed terminal grant and a permanent repository grant.
Both appear in the menu and are revoked separately. Redemption injects each
secret once. Repository grants do not renew or extend the upstream credential:
if that credential expires, update its stored value normally.

All-terminals grants carry `secret_id` and a non-empty list of program names:
no terminal, repository, or expiry. They apply to every registered PTY,
existing and future, until revoked in the Secrets tab, but only when the
command `with-cred` runs is one of the listed programs. For any other command
an all-terminals grant contributes nothing, so a terminal without its own grant
is still refused. A read-only GitHub token granted for `gh` lets every terminal
run `with-cred -- gh` for reads without asking, while `with-cred -- terraform`
still needs a grant.

Terminal and repository grants apply to every program and rank above
all-terminals grants. When both set the same environment variable, the
higher-ranked value is injected and the all-terminals value is dropped.
Granting a write-capable GitHub secret to a terminal or repository therefore
replaces the read-only `GH_TOKEN` there only.

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

Terminal and repository grants are created and revoked from terminal/session
context menus. The Secrets tab creates, updates, and deletes secret bundles and
turns a bundle's all-terminals grant on or off.

All-terminals grants widen this deliberately: that secret is reachable from
every PTY, including by any agent in any terminal. Reserve it for credentials
whose worst case is acceptable everywhere, such as a read-only token. The
program list keeps the refusal for every other command, so an agent that needs
more than the defaults is stopped and has to ask; it guards against accidental
use, not deliberate circumvention.

## Runtime use

`with-cred` at `/opt/sulion/bin/with-cred` is on the PTY `PATH`:

```sh
with-cred -- <command...>
```

It asks the broker for every secret granted to the terminal plus every
all-terminals secret that lists the command's file name, applies grant
precedence, and execs the command with those variables added to its
environment. With nothing applicable it exits `66` with the broker's reason.

## Conflict handling

`with-cred -- <command...>` may combine multiple unlocked env bundles. If two
active bundles of the same rank define the same environment variable name, the
broker rejects the request instead of silently choosing one value.

This is intentional. Secret merges must be explicit, not order-dependent. The
one ordering is by grant kind, not by time: a terminal or repository grant
replaces an all-terminals value for the same variable. Two all-terminals
secrets that set the same variable conflict.

## Runtime wiring

PTYs need these runtime values:

- `SULION_PTY_ID`
- `SULION_SECRET_BROKER_URL`
- `SULION_SECRET_BROKER_KEY_PATH`

The node injects them when it launches the PTY. `with-cred` signs each broker
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

The TrueNAS broker reads that value from SSM at startup using its workload
identity; deployment supplies identity configuration and public identifiers.
The dedicated node is not provisioned with it: the control plane forwards it,
along with the database URL and retrieval token, over the authenticated node
channel once an operator approves the node's identity key. The node writes them
to root-owned host state that the PTY identity cannot read. See
[node-protocol.md](node-protocol.md).

That channel is the node's only source of shared credentials. Roles Anywhere
enrollment is a TrueNAS-site mechanism: `backend`, `broker` and `retrieval`
read their own values from SSM with the certificate the trust appliance issued
them, and `code-intel` does the same in the standalone role, where it runs on
TrueNAS. In the split topology `node`, `ingester` and `code-intel` run on the
dedicated host, hold no AWS identity, and their containers carry no `AWS_RA_*`
variable and no enrollment wrapper.

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
- adding explicit key/value pairs, including multiline values such as SSH and PEM keys
- overwriting an existing env value without reading the old value
- **Every-terminal programs**, which creates, updates, or revokes the secret's
  all-terminals grant and its program list immediately

Existing secret values are not returned by browser read endpoints. Editing an existing bundle shows only the env key names. Leaving an existing value blank preserves it; entering a new value overwrites it.
Multiline values preserve embedded and trailing newlines through broker storage
and `with-cred` redemption. This does not change the separate host-administration
SSH key boundary described above.

Grants are managed from terminal/session context menus:

- right-click a terminal or session
- open **Secrets**
- use **Enable secret** to choose a secret and TTL, or **Always for this repository**
- use **Active secrets** to see remaining TTL or **always for &lt;repo&gt;**
- click a timed grant to revoke it for that terminal; click **revoke for repository**
  to revoke permanent access for all sessions in that repository
- all-terminals grants appear as **every terminal for &lt;programs&gt; · manage**,
  which opens the Secrets tab; enabling another secret that sets the same
  variable is allowed and supersedes it

## Broker API

Authenticated browser endpoints:

- `GET /broker/v1/secrets`
- `GET /broker/v1/secrets/:id`
- `PUT /broker/v1/secrets/:id`
- `DELETE /broker/v1/secrets/:id`
- `GET /broker/v1/grants?pty_session_id=<uuid>`
- `POST /broker/v1/grants`
- `DELETE /broker/v1/grants`

Grant creation takes `secret_id` and a `scope`:

- `terminal` (default): `pty_session_id` and `ttl_seconds` (60–86400)
- `repository`: `pty_session_id`, whose registered repository the grant covers
- `all_terminals`: `programs`, at least one command name such as `gh`

Granting again replaces the active grant for the same secret and target.
Revocation takes the same fields without `ttl_seconds` or `programs`. Grants
record their scope; grant listings include `scope` with nullable `repo`,
`expires_at`, and `programs`, keeping one entry per scope. Secret listings
include `all_terminal_programs`, null when the secret has no all-terminals
grant.

Authenticated PTY-use endpoint:

- `POST /broker/v1/use`

`/broker/v1/use` accepts signed PTY requests only. It cannot create, extend, or mutate unlock state.

Backend registration endpoints:

- `POST /broker/v1/pty-credentials`
- `DELETE /broker/v1/pty-credentials/:id`

These are authenticated with the backend registration token and are only for registering or revoking PTY public keys.

The node's CI status poller uses the same two paths: it registers its own
credential without a repository and redeems `/broker/v1/use` for program `gh`.
With no terminal or repository grant of its own, it receives only the
all-terminals secrets that list `gh`, exactly what `with-cred -- gh` receives
in a terminal, and its uses are audited like a terminal's.

## Non-goals

This system does not support:

- shell-global secret export
- direct `.env` file management
- arbitrary wrapper generation for random tools
- using broker credentials directly from the PTY
- storing the broker master key in the control, node, or PTY container
