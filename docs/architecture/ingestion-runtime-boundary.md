# Ingestion Runtime Boundary

This note closes the runtime/process evaluation part of `#48`.

## Recommendation

Keep ingestion in-process with the API server today.

If operational pressure forces a split later, split once into:

- `sulion` API server
- one `sulion-ingester` worker

Do not split into separate Claude and Codex ingestion containers unless the transcript roots, failure modes, or scaling profile diverge materially. They do not today.

## Why This Is The Right Boundary

The codebase now has clearer ownership boundaries even without a container split:

- `backend/src/ingest/canonical/` owns source-specific transcript translation
- `backend/src/ingest/timeline/` owns app-shaped timeline formation and durable projection derivation
- `backend/src/ingest/projection.rs` owns materialization into `timeline_*` tables
- `backend/src/ingest/ingester.rs` remains the transcript polling + ingest orchestration layer

That gives most of the maintainability win without adding a second runtime to operate.

The runtime boundary is different from the code boundary:

- Claude and Codex still write into the same canonical event model
- both ingestion paths share `ingester_state`, event storage, block storage, and projection rebuild behavior
- both need the same bind-mounted transcript roots and the same database
- splitting by agent family would duplicate polling/backfill/projection ownership without giving cleaner failure isolation

The meaningful future runtime boundary is therefore:

- request-serving API process
- background ingestion/projection worker

Not:

- Claude worker
- Codex worker

## Why Not Split Now

Keeping ingestion in-process is still the pragmatic choice today:

- deploy shape stays simple: one Rust service, one frontend service
- startup ordering stays simple: migrations, one-shot backfills, ingester boot, API boot
- there is no current evidence that transcript polling is saturating CPU or memory
- the `api/` vs `ingest/` split already removed the main ownership ambiguity that triggered `#48`

The remaining downside is fault isolation: a bad ingest loop can still affect the API container. That tradeoff is acceptable while load is small and transcript volume is modest.

## If We Split Later

The correct follow-up is one ingestion worker, not two.

Required changes:

- add a second Rust binary, likely `src/bin/sulion-ingester.rs`
- keep DB migrations owned by the API startup path or by a dedicated migration job, not by both containers
- move transcript polling, canonical block backfill, and timeline projection backfill/rebuild ownership into the ingester worker
- keep API containers read-only with respect to transcript JSONL
- add a new image entry in `platform.yml`
- map the new binary in `rust_artifacts.binaries`
- add an ingester service to `compose.yaml`
- mount the same transcript roots into API and worker only if both truly need them; ideally only the worker needs the transcript mounts
- keep Postgres shared between both services

## CI / Deploy Implications

The shared Ahara workflow is not the blocker.

The repo already has the right primitives for a future split:

- `platform.yml.images` can define multiple images
- `rust_artifacts.binaries` can map multiple Rust binaries to those images
- the reusable CI workflow already iterates over image definitions

The real work is ownership, not YAML:

- deciding which process owns migrations and backfills
- ensuring projection rebuilds are not run redundantly by multiple services
- defining health checks separately for API readiness vs ingester liveliness
- deciding whether the correlate socket remains API-owned or becomes its own boundary later

## Exit Criteria For A Future Runtime Split

Do the worker split when one of these becomes true:

- ingest polling or projection rebuilds measurably affect request latency
- deploy cadence for ingest logic needs to diverge from API changes
- transcript polling/backfill failures need isolated restarts and observability
- startup responsibilities in `main.rs` become operationally unsafe to keep in one process

Until then, the current module split plus lint enforcement is the right stopping point.
