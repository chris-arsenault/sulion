# ADR 0005: Browser Workspace Over a Server-Side General Shell on Shared Postgres

## Status

Accepted, 2026-04-18.

## Context

The user runs Claude Code and Codex sessions from several LAN devices:
Windows desktops, a macOS laptop, and a phone. A session has to outlive
any one of them, and attaching from a new device must not require
installing anything. Interactive use of the agent REPL has to work
exactly as it does in a native terminal; the batch streaming modes the
agents offer cannot replace the interactive TUI.

Sulion also lives inside the ahara ecosystem, which already runs a shared
Postgres on TrueNAS with credentials provisioned per project, and deploys
services as Compose stacks through Komodo. The first skeleton (0ae1d9d)
shipped a stack-local Postgres sidecar and was corrected the same day
after comparing with the sibling services.

## Decision

Sulion is a **web application**. The frontend is a React/TypeScript app
served by nginx; the browser is the only client. A phone reviews the
timeline and sends prompts from its composer; a desktop attaches to the
live terminal.

Each session is a **general shell** spawned by the server on a PTY, with
`SULION_PTY_ID` (originally `SHUTTLECRAFT_PTY_ID`) injected into its
environment. The agent is launched from that shell; a `SessionStart` hook
correlates the agent's transcript with the PTY. The user can navigate,
clone, run `claude --resume`, or do anything else in the shell. This is
what makes the PTY a workbench rather than a wrapper, and it gives the
session model its two layers: the long-lived PTY session and the agent
conversation currently running in it.

The query layer is the **shared TrueNAS Postgres**. Compose carries no
database service; `SULION_DB_URL` is built from credentials the ahara
provisioning path writes for the project. The schema is owned by SQLx
migrations in this repository.

## Alternatives considered

- **Native terminal application.** The original design document
  (`session-broker-design.md` at 0ae1d9d, "Web frontend, not native")
  weighed the native latency edge against zero-install on every LAN
  device and chose the browser; "zero per-device install friction on
  Windows LAN clients" is listed there as a forcing requirement.
  The latency concern was handled separately by keeping the terminal
  pane out of React (ADR 0004).
- **Agent-only PTY wrapper.** Running `claude` directly as the PTY
  process. Rejected in the same document ("PTY runs a general shell, not
  `claude` directly") because the user needs the shell to navigate, clone,
  and resume manually; c376811 implemented the PTY lifecycle as a forked
  shell with environment injection. The structured streaming modes
  (`--output-format stream-json`) were also considered and ruled out as
  batch-only.
- **Stack-local Postgres sidecar.** Shipped in 0ae1d9d and 3987268, with
  a CLAUDE.md invariant stating "Postgres is stack-local". Removed by
  f824752 after comparing with nas-sonarqube and airwave: the shared
  TrueNAS instance already existed, credentials are auto-provisioned, and
  the sidecar added a volume, a bootstrap chown step, and manual setup
  docs for no gain. The invariant was deleted in the same commit.
- **SQLite.** The original design chose Postgres over SQLite: the
  ecosystem runs a shared Postgres and Sulion joins it rather than carrying
  its own store.

## Consequences

- Sulion is reachable from any LAN browser with no client install;
  mobile support is a layout concern, not a second client.
- Browser clipboard conventions replace terminal ones, and keystroke
  latency depends on the WebSocket path staying out of React.
- Because the PTY is a general shell, Sulion cannot assume an agent is
  running. Correlation is event-driven (hooks, later the central launcher)
  and the UI has to represent an idle shell, an agent starting, and an
  agent running.
- All history, plans, usage, and retrieval share one database with the
  rest of the ahara estate; backup and retention (ADR 0003) are solved
  once, in Postgres, for every topology.
- The split control-plane and node topology (ADR 0002) later kept the
  database on TrueNAS and moved only the shells; this decision is why
  that split had a clean seam.

## References

- [`../architecture.md`](../architecture.md) — Shape; Session model
- [`../../README.md`](../../README.md) — Motivation
- [`../deploy.md`](../deploy.md) — shared Postgres and dataset layout
- 0ae1d9d — initial skeleton with `session-broker-design.md`
- 3987268 — Postgres schema, SQLx wiring, migrations runner
- c376811 — PTY lifecycle: spawn a shell with environment injection
- f824752 — use shared TrueNAS Postgres, drop sidecar
