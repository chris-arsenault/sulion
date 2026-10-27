# ADR 0009: A Separate Secret Broker with Per-PTY Signed Redemption

## Status

Accepted, 2026-04-24.

## Context

Agents run inside Sulion PTYs with root in the PTY container, by design: the
user wants them to work unconstrained. Anything readable from a PTY, including
process memory, mounted files, and the Sulion binary itself, is readable by an
agent. The security mechanism for credentials therefore has to sit outside the
PTY trust boundary.

Before the broker, credentials lived in `.env` files. The user's 2026-04-24
request named the problems: agents are prone to checking those files in, the
files are not as secure as wanted, and agents "reach for e.g. aws keys
whenever they want" and start interrogating AWS resources. The requirement was
a tool-managed store that is discoverable to agents while gating when a
credential is actually usable.

Sulion already mediates the world for a PTY: one backend, one Postgres, PTY
spawning under backend control, Cognito identity in the browser.

## Decision

Secrets are handled by a separate broker service in its own container with
its own uid and its own database (`sulion_broker`). The broker stores env
bundles encrypted at rest and decrypts them with a master key mounted only
into the broker container. The backend does not hold the master key and has
no route that unlocks a secret; its only broker capability is registering and
revoking per-PTY public keys.

A secret is an env bundle: one id maps to one set of environment variables.
The browser manages bundles and grants directly against `/broker/*` with the
user's Cognito JWT, and reads back metadata and key names only; stored values
are never returned.

A grant is timed and scoped to one PTY: `(pty_session_id, secret_id,
expires_at)`, created and revoked from the terminal context menu. Redemption
happens only through `with-cred` inside the PTY, which asks the broker for the
granted bundles and execs the command with those variables added to the
child's environment. Nothing is exported into the shell.

The first release (c20f9d8) authenticated wrappers with one shared
`SULION_SECRET_BROKER_USE_TOKEN`. v1.0.0 (af28824, 2026-05-02) replaced it
with a per-PTY Ed25519 keypair: the backend registers the public key with the
broker at PTY creation, the private key is written under
`/run/sulion/pty-keys/`, every `/v1/use` request is signed with a nonce, and
the registration is revoked on PTY shutdown. `/v1/use` cannot create, extend,
or mutate unlock state.

The scope guarantee is deliberately narrow: a grant separates locked from
unlocked, not one terminal from another. Every PTY runs as the same uid, so
the per-PTY key files are readable across terminals. The signature proves a
request came from this node and attributes it for audit; it does not contain a
hostile agent to its own terminal.

## Alternatives considered

- **`.env` files in repositories.** The starting point. Rejected by the user
  at the outset: prone to being committed by agents, not secure enough, and no
  gate on when an agent may use a credential (session
  `019db1c2-323d-7e50-a6e2-aedacb25f083`, turn 3440948).
- **Encrypted secrets in the main Postgres, decrypted by the backend.** The
  first proposal: a `secrets` table in Sulion's database and a host-local
  master key read only by the backend container (turn 3440948). Rejected at
  turn 3621913 once the PTY root boundary was stated: if the process that can
  decrypt shares a trust boundary with the PTY, the key is reachable. The user
  also required a separate database user. Baking the key into the Sulion
  binary was rejected in the same turn for the same reason: a root-capable
  agent can read the binary off disk, dump process memory, or ptrace the
  process.
- **Injecting secrets into the running shell's environment.** Considered as a
  "right-click terminal, enable secret, variable appears" model (turn
  3531589). Mutating a running PTY's environment was ruled out by the user
  ("insane talk"); the fallback of a `.bashrc` hook sourcing a per-PTY env
  fragment is bash-specific, applies only at prompt boundaries, and leaves the
  secret in the shell for the PTY's lifetime. The exec-only `with-cred` model
  was chosen instead; the shell-global export is a documented non-goal.
- **One shared static use token for all wrappers.** Shipped in c20f9d8.
  Rejected at turn 7884612 because a single token redeems grants for every
  terminal and there is no per-terminal attribution; replaced by per-PTY
  signing keys with nonce replay protection in af28824.

## Consequences

- A further container, database, migration set, and a backend-to-broker
  registration token to distribute. ADR 0002 keeps the broker and master key
  on TrueNAS and forwards only the registration token to the node.
- Agents have to run `with-cred -- <command>`; a plain `aws` or `gh` with no
  grant fails. Secrets are discoverable by id and key name in the UI but their
  values are never readable there.
- Grant scoping limits what is live at a moment and records who asked. It is
  not inter-terminal isolation; `docs/secrets.md` states that anything
  unlocked in one terminal should be treated as reachable from another.
- Later grant kinds (permanent repository grants, program-bound every-terminal
  grants) and the collapse to a single credential path are ADR 0028.

## References

- Secret boundary and grant model: [`../secrets.md`](../secrets.md)
- Broker surface: [`../architecture.md`](../architecture.md#broker-surface)
- Topology placement of the master key: [ADR 0002](0002-hybrid-control-plane-and-dev-node.md)
- Grant scopes and the single credential path: [ADR 0028](0028-with-cred-as-the-only-credential-path.md)
- Commits: c20f9d8 (broker and management UI), b4242e0 (secrets
  documentation), af28824 (v1.0.0: per-PTY signed redemption)
- Transcript: session `019db1c2-323d-7e50-a6e2-aedacb25f083`, turns 3440948,
  3531589, 3621913, 3704838, 7884612
