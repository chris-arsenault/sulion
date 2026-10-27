# ADR 0029: DORA Metrics Are Aggregated from the Known Deploy Pipeline's Evidence, Portfolio-Wide Only

## Status

Accepted, 2026-10-15; scale and aggregation correction 2026-10-16.

## Context

Metrics already showed token spend, flow, file churn, and plan burndowns.
The user asked for rolling 30-day DORA metrics beside the burndowns:
deployment frequency, change lead time, change failure rate, failed
deployment recovery time, and deployment rework rate.

Sulion had no record of deployments. What it did have was the node's
repository manager, which already polled GitHub through a brokered
`GH_TOKEN` with rate-limit pausing, and node-local Git history. The
repositories the user cares about deploy through one known pipeline: a
main-branch GitHub Actions run with the Ahara `deploy-truenas` action, or a
`Deploy to TrueNAS`, `Deploy to Komodo`, or `Terraform deploy` step. Other
repositories have workflows that build and test without deploying
anything, and a green run there is not a deployment.

The first release (`c2136ca`, 2026-10-15) showed a per-repository table with
per-repository metrics and a repository selector, and returned deployment
detail inside the routine metrics poll. The user has about 50 repositories.
On 2026-10-16 the display did not fit and the eager payload was too large;
the user clarified that the metrics must be aggregates, not per-repository
figures, and the correction landed as `b82fef7`.

## Decision

Deployment evidence comes only from the known pipeline. The node's
repository manager collects Actions run and job history alongside its
status loop, every 30 minutes, using the same `GH_TOKEN` grant and
rate-limit pause as `gh`. A main-branch push or manual run with a recognised
deploy step is a production deployment; a TrueNAS deployment takes
precedence over its preparatory Terraform apply; a successful deploy step
counts even when a later reporting step fails. Any other pipeline yields
no deployments, and the repository's metrics read as unavailable rather
than zero. Collection errors keep the last evidence and are visible in
deployment details.

`dora_deployments` stores run-attempt identity, so re-polling and separate
attempts do not duplicate a deployment, plus an explicit production
outcome: unknown, healthy, or failed, with failure and recovery timestamps
and an unplanned-incident-rework flag. Operators set the outcome through
`PUT /api/metrics/deployments/:id/outcome`; polling preserves annotations,
repository renames carry them, and the durable archive dump includes both
tables. `dora_commit_deployments` records each commit's first observed
production deployment from node-local Git ancestry. The earliest observed
deployment is a baseline and claims no historical lead time; missing local
Git objects leave lead time unknown.

Metrics are portfolio-wide aggregates. The page shows one rollup across all
repositories: five tiles in Delivery flow above the burndowns, with no
repository table, selector, or per-repository panels. Portfolio medians are
computed from the underlying commit and incident samples, not by averaging
repository medians. Failure and rework rates stay unknown until every
deployment in the window is classified; unknown is rendered distinctly from
a measured zero, and the summary shows collection coverage.

The metrics poll returns summaries only. Deployment details load on demand
through `GET /api/metrics/deployments` in 25-record pages, optionally scoped
by repository, with a deployment-time and id cursor. Indexed lookups
restrict commit aggregation to deployments in the reporting window. Saving
an outcome refreshes DORA through `GET /api/metrics/dora` without reloading
the other metrics. REST reads Postgres only; the node is the only GitHub
client.

## Alternatives considered

- **Per-repository tables, selection, and metric panels.** The first
  release. Removed in the 2026-10-16 correction recorded in the plan: the
  table did not fit 50 repositories, and the user clarified that the
  metrics must be aggregates. The plan's original contract already
  excluded "team rankings or misleading portfolio medians"; the correction
  resolved that by computing medians from samples rather than by keeping
  per-repository views.
- **Eager deployment payloads in the metrics poll.** The first release
  returned deployment detail with the summary. Removed in the same
  correction because the payload scaled with deployments across 50
  repositories on every poll; a 50-repository fixture with 60,000
  deployments and 300,000 commits was used to size the replacement.
- **Inferring deployments from unknown pipelines.** Treating any successful
  main-branch workflow as a deployment would have produced frequency
  numbers for repositories that never deploy. The contract states "Unknown
  pipelines produce no invented deployments" and the shipped behaviour
  reports them as unavailable.
- **Treating CI failures as incidents.** A failed build or test run is not
  a production failure and would inflate the change failure rate. The
  contract states "CI failures are not production incidents"; failures are
  explicit operator annotations on a deployment, and the rate is unknown
  until annotations are complete.

## Consequences

- DORA numbers exist only for repositories on the known pipeline. Adding a
  pipeline means adding a recogniser, not loosening the inference.
- Change failure and rework rates stay unknown until the operator
  classifies every deployment in the window. That is the intended cost of
  not inventing outcomes; the UI shows coverage and unresolved failures so
  the gap is visible.
- Lead time starts at the first observed deployment per repository, so the
  first windows after enabling collection say nothing about commits that
  shipped before it.
- The metrics page cannot answer per-repository DORA questions. A reader
  who needs one repository's deployments pages them in the detail view;
  per-repository rates are not computed.
- History collection depends on the every-terminal `GH_TOKEN` grant for
  `gh`. Without it, evidence stops advancing and the last collection error
  is shown.
- The 2026-10-16 correction was verified against a fixture only; live
  profiling was unavailable because the credential broker refused database
  access, and deployed behaviour was recorded as unverified.

## References

- Shipped mechanism: [`../architecture.md`](../architecture.md), "Delivery
  metrics"
- Published Sulion plans `344ba0e1-d813-4c88-a6e5-0aabca5444bb` "DORA
  metrics beside burndown", `4f47a6a2-9d7a-4e30-865c-d1d600111dfd` "Fix
  DORA metrics at 50 repositories", and
  `7da5d46b-b88f-44d2-a18b-463bb19785c5` "Make DORA aggregate-only"; the
  working contract and the 2026-10-16 scale correction are folded into
  this ADR
- Change record: `CHANGELOG.md`, Unreleased, "DORA metrics"
- Metric definitions: <https://dora.dev/guides/dora-metrics/>
- Commits: `c2136ca` — Add DORA metrics beside plan burndowns; `b82fef7` —
  Aggregate DORA metrics and page deployment history
