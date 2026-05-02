<img src="frontend/public/favicon.svg" alt="sulion favicon — tengwa silmë with u-tehta (sú)" width="64" height="64" align="left" />

# sulion

*Quenya **súlë** (breath, spirit, emanation) bound to Aulë, Vala of craftsmanship — "vibe-forge."*

Session broker for Claude Code and Codex terminal sessions. Persistent PTYs with structured timeline scrollback, served to any LAN device via web UI.

## Motivation

- **Terminals viewable from anywhere on the LAN.** A long-running agent session shouldn't be chained to one laptop. Sulion parks the PTY on a server and lets any LAN device — desktop, laptop, phone — attach, watch, and type into the same live shell with a snapshot-on-connect so you never land in an empty buffer.
- **Non-linear workflows.** Real agent work isn't one prompt at a time — you queue follow-ups while a turn is running, review a past turn while another is in flight, and jump between sessions mid-thought. The tab system, future-prompt queue, and repo timeline exist so the UI doesn't force a single linear thread.
- **Integrated Development Terminal.** An IDE is built around the editor; an IDT is built around the agent's terminal. File tree, git diff, file tabs, library, and timeline all sit alongside the live PTY so the agent's interactive shell is the product, not a panel bolted to something else.
- **Agent timeline as source of information.** The JSONL transcript is authoritative — the terminal is ephemeral. Every turn, tool call, edit, and thought is projected into a structured, filterable timeline you can review, filter by file, trace references from, and share as markdown. Reviewability is a first-class feature, not a scrollback consolation prize.

## Docs

- [User guide](docs/user-guide.md) — feature tour with screenshots
- [Architecture](docs/architecture.md) — shape, session model, invariants
- [Ingestion runtime boundary](docs/ingestion.md) — why ingest runs in-process, when to split
- [Design (visual framework)](docs/design.md) — IDT tokens, primitives, tiers
- [State management](docs/state-management.md) — Zustand + app command layer rules
- [Development](docs/development.md) — local dev, make targets, test contracts
- [E2E coverage plan](docs/e2e-coverage-plan.md) — real-stack Playwright suite
- [Deploy](docs/deploy.md) — TrueNAS / Komodo first-run and ongoing
- [Backlog](docs/backlog.md) — active candidates and speculative bets
- [Changelog](CHANGELOG.md) — user-visible feature history

## Quickstart

```bash
make ci    # full lint + unit + backend integration
make e2e   # Playwright against real backend + Postgres + seeded ingest
```

See [docs/development.md](docs/development.md) for running the services and [docs/deploy.md](docs/deploy.md) for TrueNAS setup.

## License

MIT — see [`LICENSE`](LICENSE).
