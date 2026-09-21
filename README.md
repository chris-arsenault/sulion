<img src="frontend/public/favicon.svg" alt="sulion favicon — tengwa silmë with u-tehta (sú)" width="64" height="64" align="left" />

# sulion

*Quenya **súlë** (breath, spirit, emanation) bound to Aulë, Vala of craftsmanship — "vibe-forge."*

Browser workspace for Claude Code, Codex, and the Fugu Codex profile. Sulion
combines persistent server terminals, transcript review, repository tools, and
agent supervision. A Rust backend and React frontend share Postgres-backed
history; the standard deployment separates the control plane from a dedicated
development node.

## Motivation

- **Sessions independent of one laptop.** Sulion keeps the shell on the server.
  Desktop browsers attach to the live terminal with a snapshot on connect;
  phones review the timeline and send prompts from its composer.
- **Non-linear workflows.** Real agent work isn't one prompt at a time — you queue follow-ups while a turn is running, review a past turn while another is in flight, and jump between sessions mid-thought. The tab system, future-prompt queue, and repo timeline exist so the UI doesn't force a single linear thread.
- **Integrated Development Terminal.** An IDE is built around the editor; an IDT is built around the agent's terminal. File tree, git diff, file tabs, library, and timeline all sit alongside the live PTY so the agent's interactive shell is the product, not a panel bolted to something else.
- **Reviewable agent history.** Transcript events are retained in Postgres and
  projected into a filterable timeline. Review tool detail, trace touched
  files, and copy a turn digest without depending on terminal scrollback.

## Primary features

- **Persistent sessions.** Reconnect to a populated terminal buffer and resume
  agent history. On the dedicated node, control and node releases preserve
  running shells. Toolset upgrades restart only the session you choose.
- **Structured timeline.** Review prompts, assistant text, thinking, tool
  calls, edits, and child-agent logs. Filter by speaker, category, errors, or
  touched file; follow source links into file tabs and copy a turn digest.
- **Flexible reading and input.** Use split, terminal-only, or timeline-only
  layouts on desktop; mobile uses the timeline. Send or steer prompts,
  interrupt a turn, recover unmatched submissions, and review unexpected
  model switches from the timeline.
- **Repository workspaces.** Browse files, trace them to turns, inspect and
  stage diffs, and choose a canonical checkout or isolated worktree. Group
  related repos and launch collection sessions over their canonical roots.
- **Plans and supervision.** Publish phases and branch plans, report blocked
  or needs-input states, and see live sessions in Overview. Metrics shows
  token categories, API cost estimates, Git activity, file churn, and plan flow.
- **Reusable context.** Save templated prompts and references, queue session
  follow-ups, and give agents transcript retrieval and structural code
  navigation through `sulion-retrieve` and `sulion-code`.
- **Brokered credentials.** Manage encrypted environment bundles and timed
  PTY grants. `with-cred` injects enabled credentials into one command; the
  `aws` wrapper uses the same grants.

## Docs

- [User guide](docs/user-guide.md) — sessions, timeline controls, prompts, and keyboard shortcuts
- [What sulion does](docs/agentic-practice.md) — the system mapped onto current agentic-development practice
- [Architecture](docs/architecture.md) — shape, session model, invariants
- [Ingestion](docs/ingestion.md) — transcript ownership, projections, repair, and usage accounting
- [Meta-repositories](docs/meta-repositories.md) — logical groups and collection sessions
- [Plans and activity](docs/plans.md) — published phases, branches, and terminal status
- [Retrieval](docs/retrieval.md) — search, turn lookup, and file history for agents
- [Code intelligence](docs/code-intel.md) — structural navigation and semantic references
- [Secrets](docs/secrets.md) — credential bundles, grants, and runtime boundaries
- [Design (visual framework)](docs/design.md) — IDT tokens, primitives, tiers
- [State management](docs/state-management.md) — Zustand + app command layer rules
- [Development](docs/development.md) — local dev, make targets, test contracts
- [E2E coverage plan](docs/e2e-coverage-plan.md) — real-stack Playwright suite
- [Deploy](docs/deploy.md) — TrueNAS / Komodo first-run and ongoing
- [Agent instructions](docs/agent-instructions/README.md) — user-wide CLAUDE.md / AGENTS.md templates that teach agents the sulion tooling
- [Backlog](docs/backlog.md) — active candidates and speculative bets
- [Implementation plans](docs/plans/README.md) — pending proposals and completed records
- [Changelog](CHANGELOG.md) — user-visible feature history

## Development checks

```bash
make ci                     # lint, unit tests, and type checks
make test-rust-integration   # isolated Postgres-backed integration suite
make e2e                    # Playwright against the real stack + seeded ingest
```

See [docs/development.md](docs/development.md) for running the services and [docs/deploy.md](docs/deploy.md) for TrueNAS setup.

## License

MIT — see [`LICENSE`](LICENSE).
