# ADR 0006: Canonical Event Schema With Per-Agent Parsers

## Status

Accepted, 2026-04-19.

## Context

The first timeline (75c9372) normalised Claude Code's JSONL shape in the
frontend: `timeline/types.ts` probed `payload.message.content` for text,
tool uses, and tool results, and tool renderers switched on Claude's tool
names (`Read`, `Edit`, `Bash`). Every consumer of history understood one
agent's private format.

Two pressures arrived together. Claude Code's transcript format is not a
stable public API and drifts between releases, and a second agent, Codex,
was about to be plugged in with a different record shape and different
tool names (`read_file` rather than `Read`). Letting each renderer and
route know both formats would have doubled the surface that breaks on
drift.

## Decision

The ingester normalises each JSONL record into an **agent-agnostic
canonical event**: `agent`, `speaker`, `content_kind`, and an ordered list
of blocks (text, thinking, tool use, tool result, and so on) with
**canonical tool names**. The database is the integration point. REST
handlers and frontend renderers read canonical blocks and never reach into
an agent-specific payload shape.

Each agent has one parser behind an `EventParser` trait
(`backend/src/ingest/canonical/`). The Claude parser is a faithful mapping
of the current JSONL shape; the Codex parser landed first as a stub proving
the extension point and now carries the full Codex rollout format. Unknown
tools fall back to a snake_case canonical name; unknown block types are
preserved rather than dropped. The raw payload stays in `events.payload`
untouched as the forensic record and the source for re-derivation.

One ingester serves both agents. Claude and Codex share `ingester_state`,
event storage, block storage, and the projection pipeline; the ingester
receives both read-only transcript roots. Canonical and timeline
projections are versioned independently, and a version bump repairs the
affected projection from stored payloads instead of replaying files.

CLAUDE.md states the rule for agents editing ingest code: Codex and Claude
share the same canonical event schema; assume both flow through it.

## Alternatives considered

- **Renderers and routes reading agent-specific payloads.** The prior
  state. The e506453 commit body describes the problem it replaced: the
  history endpoint and the frontend helpers read Claude's payload shape
  directly, so a Claude format change or a second agent touched every
  consumer. After the change "only the parser file changes". The legacy
  payload probing was kept briefly as a fallback for rows not yet
  backfilled, then removed.
- **A separate ingester per agent family.** `docs/ingestion.md`
  (Ownership) records the reason: splitting by agent would duplicate
  ownership of state, storage, and projection without buying failure
  isolation, because both agents' files are local to the same node and
  both write into the same tables.
- **Normalising in the frontend only.** This is what 75c9372's
  `timeline/types.ts` did. e506453 moved normalisation server-side so the
  database, retrieval, usage, and file-touch projections all see one
  shape; the frontend helpers kept their API but began sourcing from
  blocks.

## Consequences

- Adding or updating an agent means writing or revising one parser and
  bumping the canonical projection version; renderers, filters, retrieval,
  and usage are unchanged. Fugu arrived later as a Codex profile with no
  schema work.
- Raw payloads are retained indefinitely in Postgres as the rebuild
  source, which is the storage cost ADR 0003 later addressed with tiered
  retention; archive restore replays through the same insert path.
- Canonical names are what the UI and filters show by default; the
  agent-specific original is kept as `rawName` for display.
- A record a parser does not understand is stored and skipped, not
  rejected, which keeps invariant 3 (tolerate unknown event types) true
  for both agents.

## References

- [`../ingestion.md`](../ingestion.md) — Ownership; Derived repair and
  background jobs
- [`../architecture.md`](../architecture.md) — Invariants 1, 3, 5
- [`../../CLAUDE.md`](../../CLAUDE.md) — Working rules
- 75c9372 — first timeline pane, frontend-side Claude normalisation
- e506453 — canonical event schema and parser trait
- 69f99b2 — Codex ingestion through the canonical model
