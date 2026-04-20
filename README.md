# sulion

*Quenya **súlë** (breath, spirit, emanation) bound to Aulë, Vala of craftsmanship — "emanation-forge."*

Session broker for Claude Code and Codex terminal sessions. Persistent PTYs with structured timeline scrollback, served to any LAN device via web UI.

## Docs

- [Architecture](docs/architecture.md) — shape, session model, invariants
- [Ingestion runtime boundary](docs/ingestion.md) — why ingest runs in-process, when to split
- [Design (visual framework)](docs/design.md) — IDT tokens, primitives, tiers
- [State management](docs/state-management.md) — Zustand + app command layer rules
- [Development](docs/development.md) — local dev, make targets, test contracts
- [E2E coverage plan](docs/e2e-coverage-plan.md) — real-stack Playwright suite
- [Deploy](docs/deploy.md) — TrueNAS / Komodo first-run and ongoing
- [Backlog](docs/backlog.md) — active candidates and speculative bets
- [Changelog](docs/changelog.md) — user-visible feature history

## Quickstart

```bash
make ci    # full lint + unit + backend integration
make e2e   # Playwright against real backend + Postgres + seeded ingest
```

See [docs/development.md](docs/development.md) for running the services and [docs/deploy.md](docs/deploy.md) for TrueNAS setup.

## License

MIT — see [`LICENSE`](LICENSE).
