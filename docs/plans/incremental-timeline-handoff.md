# Fresh-agent handoff: incremental timeline writes

Prepared 2026-09-24. Read [the replacement contract](incremental-timeline-projection.md)
before implementation. The user rejected the previous attempt as overcomplicated
and requested that it be stashed, the environment reset, and a fresh handoff made.

## Starting point

- Repository: `/home/sulion/repos/sulion`; branch: `main`.
- Application baseline: `7b799efefd54bc64a745bfa916c37f4f8eed8f7f`. The restart
  commit adds documentation only, at the user's request. No branch was created,
  no existing commit was removed, and nothing was pushed or deployed.
- Abandoned attempt: stash `2e730845f0470300d7814cc8d8d320019e9dcd00`, initially
  `stash@{0}`, named `sulion abandoned incremental timeline attempt 2026-09-24;
  reference only, do not apply wholesale`. Use the hash; stash numbering can change.
- All 81 affected files, including 37 untracked files, were verified against the
  stash by Git blob hash. The stash contains 7,598 additions and 198 deletions.
- Application source and migrations were restored to the baseline by the scoped
  stash. This handoff and the replacement plan are the only new task documents.
- Unrelated edits in `docs/agent-instructions/AGENTS.md` and
  `docs/agent-instructions/CLAUDE.md` were preserved byte-for-byte: ten added lines
  each about plan guidance. The user then authorized committing the current dirty
  tree, so these edits are included in the documentation commit, outside the stash.
- No test runner or test container from this attempt remains running. No E2E
  network remains. Existing services, other agents, older Docker volumes,
  dependencies and ignored build caches were left alone. Rebuild before relying
  on a local executable; cached binaries can contain the abandoned implementation.
- No new projection schema was applied to the deployed database. Tests used
  isolated disposable databases. The reset did not modify production data.

## Plan and next action

Attach plan `3023d7e5-89ab-4c29-9506-032477ecbb85` in the new terminal after reading
`sulion plan help`, then read `sulion plan current`. All three implementation
milestones are pending. The old root `e2029cd3-13a7-4e00-9de1-95adacfef341` and its
unfinished branches were canceled; its historical completed M1 is not present
in the reset checkout.

The first task is to map the minimum missing durable state onto the existing
tables, then implement the affected-record write path. Reuse the existing turn
UI and change its data loading as needed. A few tables/columns and a narrow API
change are the intended scale. Preserve correctness without recreating the
stashed generation, compatibility, queue and renderer machinery.

The previous implementation authorization remains the feature context; this
handoff turn performed only reset and planning, followed by the user-authorized
documentation commit. Future implementation commits, pushes and deployment need
their own authorization. Do not apply the stash wholesale. Read any candidate with `git show`,
then make selected project edits through the native editing tool.

## Verified bottleneck

At 20:12–20:15 UTC on September 23, PTY
`1b19da82-bc0c-459b-afb1-0d0533266b27`, Claude session
`f0a7362a-5e05-49ae-b173-1555431f7a30`, showed about 90 seconds from final response
to ingestion checkpoint, then 11.8 ms to timeline-state update. Four child files
were processed first; each reconciled the same roughly 15,000-event,
3,927-operation parent. The serial ingestion loop awaited those projections,
and previously captured file lengths could defer a new response another cycle.

Detailed historical evidence remains at
`/tmp/sulion-timeline-delay-1b19da82-findings.md`. Its original recommendation to
queue/coalesce whole projections is superseded: ordinary updates must stop
rebuilding history, not merely schedule the rebuild differently. These timings
do not establish the current deployment state.

Start reading `backend/src/ingest/projection/write.rs`,
`backend/src/ingest/projection.rs`, `backend/src/ingest/timeline/derived.rs`,
`backend/src/ingest/ingester.rs`, the existing migrations, and `TurnDetail.tsx`.
Follow repository instructions and `sulion-code help` for structural navigation.
Do not re-diagnose production before addressing the established code mechanism.

## Small parser fix worth recovering independently

The reported `Uncorrelated runtime evidence: FileChange` spam had a separate,
verified cause. `timeline/runtime.rs::running_cell` searched anywhere in tool
output for `Script running with cell ID `. Completed tool outputs displaying
that source string were mistaken for running scripts. Three stale execution
intervals made the actual file change appear ambiguous.

The stashed fix recognizes that signal only at the start of the first output
line. It also classifies unmatched evidence as bookkeeping and excludes the
diagnostic from normal conversation/digest text, retaining the evidence.
Inspect these tracked-file diffs against the stash's first parent:

- `backend/src/ingest/timeline/runtime.rs`
- `backend/src/ingest/timeline/types.rs`
- `backend/src/ingest/timeline/render.rs`
- `backend/src/ingest/timeline/tests.rs`

Do not recover the modified `timeline/mod.rs` wholesale; it also exposes APIs
for the abandoned implementation. The new incremental module is not needed for
this parser correction. The baseline still has the bug because the fix was
stashed intentionally with the rest of the attempt.

## Evidence to reuse, not inherit as a passing gate

- Twenty timeline unit tests passed after the parser fix, including a quoted
  running-header regression and bookkeeping visibility checks.
- Three incremental-runtime integration tests passed, but depend on the rejected
  schema. Recover their behavioral cases rather than their table assertions.
- An earlier snapshot passed 179 backend integration tests. Later edits were
  not covered by that full run. This is not validation of the final stash.
- Useful cases in stashed untracked tests: a late result after a newer prompt,
  downward usage correction, result before call, reused/ambiguous identities,
  duplicate replay, and independent child ownership. Untracked test files are
  in the stash's third-parent tree (`<stash-hash>^3:<path>`).
- Earlier append measurements kept SQL-call counts fixed across larger turns,
  but buffer work exposed a broad join; its later rewrite was not remeasured.
  Single backend latency samples at 1/8/32 busy sources were not browser p95/p99
  or proof of nested-tree capacity. Do not carry them forward as acceptance.
- The browser run failed during E2E stack startup before any assertions ran.
  Logs showed the broker database creation overlapping initial Postgres shutdown;
  do not treat that observation as a confirmed harness root cause or fix it
  speculatively as part of this feature.

Use existing test entry points. In direct Docker mode, integration tests run
through `scripts/run-backend-integration-tests.sh`; never use production for
tests or invoke `sulion postgres`. The discarded harness added optional target,
profile and statistics switches; those switches are absent from the baseline.
For authorized live queries, use `with-cred --` and the broker-injected database
credentials. Stop on credential failures. No new production query is needed
to begin the replacement implementation.
