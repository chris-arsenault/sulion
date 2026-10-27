# ADR 0008: End-to-End Tests Run Against the Real Stack With Deterministic Mock Agent Launchers

## Status

Accepted, 2026-04-20. Reaffirmed 2026-10-17 when the suite was wired
into CI, and again on 2026-10-21 when that wiring was removed and the
suite kept its shape as a local gate.

## Context

Most of Sulion's value is in paths that cross process boundaries: a shell
starts an agent, the agent writes JSONL, the ingester projects it, the
browser polls it, and the person reads it next to a live terminal. Unit
tests on either side of those boundaries cannot show that the whole path
works, and the Claude and Codex transcript formats are exactly where drift
appears.

The real agents cannot be part of a test. They spend tokens, their output
is non-deterministic, and they need credentials the test harness must not
hold.

## Decision

E2E is **one Playwright suite** in `frontend/e2e/`, run by
`scripts/run-e2e-stack.mjs` against the **real stack**: the backend,
broker, node, ingester, and a Postgres container, with the frontend served
by its built nginx image including the production entrypoint, proxy routes,
and security headers. The harness seeds Claude and Codex JSONL into
isolated transcript roots, lets the ingester correlate and project it, and
drives the UI.

Agents are replaced by **deterministic mock launchers**. A spec opens a
real shell session, runs the mock `claude` or `codex` launcher from it,
and the launcher emits Claude- or Codex-shaped assistant, edit, websearch,
and subagent records into the transcript root. The same spec then verifies
the ingested result in the timeline. Live agent execution is listed in
`docs/e2e-coverage-plan.md` as out of scope by requirement.

There is **no in-browser mock mode**: CLAUDE.md's working rules state the
suite is the single Playwright suite "backed by `scripts/run-e2e-stack.mjs`.
No in-browser MSW mock mode." Test servers and containers are temporary and
are cleaned up when Playwright exits.

## Alternatives considered

- **In-browser request mocking (MSW).** Ruled out as a rule in CLAUDE.md
  and AGENTS.md when the docs were consolidated (5464e88), the same day
  the real-stack harness landed (40b6d10). A frontend-only mock would
  bypass the ingester and the projection path, which are what the
  coverage plan says the launcher-backed roundtrip exists to exercise
  ("direct browser coverage for Claude/Codex ingest parity without
  calling the real agents").
- **Live, token-using agents in the suite.** `docs/e2e-coverage-plan.md`
  marks "Actual live agent integration" as `Out of scope`, "intentionally
  excluded by requirement", with the instruction to keep it excluded
  unless requirements change.
- **Unit and backend-integration coverage only.** The coverage plan
  treats areas that have only unit or integration coverage (overview,
  published plans, workspace isolation) as `Partial` with a named browser
  scenario still to add, which is the standing judgement that the
  cross-boundary path needs a browser test.
- **Gating CI and deployment on the browser suite.** Tried in 474cab1:
  `platform.yml` opted into the shared ahara browser runtime and the
  Playwright web-server timeout rose to 20 minutes for cold Rust and
  Docker builds. Reverted in d364c1f after the user objected to diverging
  the shared CI workflow for one project ("why not just go back to how it
  was before"). The suite runs through `make e2e` and does not gate
  deployment; its contents did not change.

## Consequences

- A passing run means the production frontend image, nginx headers, the
  node protocol, ingestion, and the UI agree on one commit. Spec 10 checks
  the security policy against the real headers for that reason.
- The suite is slow to start cold and depends on Docker, which is why it
  is not in the shared CI job. Browser regressions are caught locally,
  before a push, or not at all.
- New agent-facing behaviour needs a mock launcher fixture that emits the
  new record shape, so the fixtures are a second place transcript format
  knowledge lives.
- There is no cheap "frontend only" test loop for UI work; component tests
  under Vitest fill that role.

## References

- [`../e2e-coverage-plan.md`](../e2e-coverage-plan.md)
- [`../development.md`](../development.md) — E2E; CI
- [`../../CLAUDE.md`](../../CLAUDE.md) — Working rules
- 40b6d10 — dockerized mock agent e2e coverage
- c1111a7 — integration harness and repo timeline
- 5464e88 — docs consolidation carrying the "no MSW mock mode" rule
- 474cab1 — gate releases on real-stack browser journeys
- d364c1f — restore CI without the browser matrix opt-in
