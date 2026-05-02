# Ingestion runtime boundary

Shipped decision from #48: ingestion runs in-process with the API server. If operational pressure forces a split later, split once — one API service, one ingester worker — not per agent family.

## Current boundary

Code ownership is already split even though the runtime is not:

- `backend/src/ingest/canonical/` — source-specific transcript translation
- `backend/src/ingest/timeline/` — app-shaped timeline formation and projection derivation
- `backend/src/ingest/projection.rs` — materialization into `timeline_*` tables
- `backend/src/ingest/ingester.rs` — polling + orchestration

Claude and Codex write into the same canonical event model, share `ingester_state`, event storage, block storage, and projection rebuild behavior, and need the same bind-mounted transcript roots. Splitting by agent family would duplicate ownership without buying failure isolation.

## Why not split today

- Deploy stays one Rust service, one frontend service.
- Startup opens the API after database migrations and orphan reconciliation. Derived transcript repair is gated by `ingest_projection_versions`; when a parser/projection version is behind, Sulion repairs missing canonical/timeline fields from existing Postgres `events.payload` rows before starting the ingester. It does not replay JSONL on ordinary startup.
- No evidence transcript polling is saturating CPU or memory.
- The `api/` vs `ingest/` code split already removed the ambiguity that triggered #48.

Accepted downside: a bad ingest loop can still affect the API container. Fine while load is modest.

## Exit criteria for a runtime split

Do the split when one of these becomes true:

- Ingest polling or projection rebuilds measurably affect request latency.
- Deploy cadence for ingest logic needs to diverge from API changes.
- Transcript polling / backfill failures need isolated restarts and observability.
- `main.rs` startup responsibilities become operationally unsafe in one process.

## Split shape when it happens

One new binary: `src/bin/sulion-ingester.rs`. Required changes:

- Move transcript polling, canonical backfill, and projection backfill/rebuild ownership into the worker.
- Keep migrations owned by one side (the API, or a dedicated migration job) — not both.
- Keep API containers read-only with respect to transcript JSONL.
- Add an image entry in `platform.yml`, map the binary in `rust_artifacts.binaries`, add an ingester service to `compose.yaml`.
- Mount transcript roots only where they're actually needed (ideally worker-only).
- Keep Postgres shared.

The real work is ownership, not YAML: decide which process owns migrations and backfills, prevent redundant projection rebuilds, and define API-readiness vs ingester-liveness separately.
