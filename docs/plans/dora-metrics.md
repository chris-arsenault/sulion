# DORA metrics beside burndown

Add repository-level, rolling 30-day DORA metrics inside Delivery flow beside
the existing plan burndowns. Implementation, tests, commit and push are
authorized. Push uses the current branch and the existing release pipeline.

## Contract

- Persist successful production deployments from the known Ahara main-branch
  GitHub Actions deployment steps. Read all recent runs and their jobs, including
  runs whose later reporting steps failed. CI failures are not production
  incidents. Unknown pipelines produce no invented deployments.
- Reuse the node's existing GitHub client, broker credential, rate-limit pause
  and repository manager. History collection runs alongside status polling so
  initial backfill does not block sidebar status. REST metrics reads Postgres only.
- Keep deployment identity stable across polls and separate run attempts.
  Collect commit ancestry on the node to measure each commit's first production
  deployment. The first observed deployment establishes a baseline and does not
  claim historical lead times. Missing local Git objects leave lead time unknown.
- Record production outcome explicitly on each deployment: unknown, healthy,
  or failed, with failure/recovery timestamps and whether the deployment was
  unplanned incident rework. Expose these annotations in Metrics.
- Report deployment frequency, median change lead time, change failure rate,
  median failed deployment recovery time and deployment rework rate per repo.
  Failure/rework rates remain unknown while any deployment in the window lacks
  the corresponding classification. Expose coverage and unresolved failures.
- Place DORA in Delivery flow near burndown, preserve existing charts and use
  existing visual tokens. No team rankings or misleading portfolio medians.

The provisional automatic source is the known Ahara pipeline on main. The user
specified placement near burndown and has not selected a different source.
Source definitions: https://dora.dev/guides/dora-metrics/ and GitHub's Actions
workflow runs/jobs REST API documentation.

## Milestones

Root Sulion plan: 344ba0e1-d813-4c88-a6e5-0aabca5444bb.

### M0 — Collect and aggregate DORA evidence

Persist deployment, commit and annotation data; collect it on the node and
aggregate it through the existing metrics response. Accept when idempotency,
pagination, deployment-step selection, first-deployment lead time, unknown
classifications and recovery boundaries have meaningful coverage.

Execution expansion: f9b22fad-ab51-42ec-8977-9d7c581cea65.

1. Add storage, aggregation and outcome validation in a dedicated DORA module;
   wire authenticated outcome updates and repository rename handling.
2. Add the GitHub Actions collector using the existing client and scheduling;
   collect Git ancestry on the node. Verify against isolated HTTP/Git/Postgres
   fixtures, including a successful deploy followed by failed reporting.

### M1 — Show DORA beside burndown [depends on M0 API contract]

Add typed DORA response data, per-repository summaries and deployment outcome
editing in Delivery flow. Accept when Metrics renders real values, unknown
coverage and mutation failures without disrupting existing burndowns.

Execution expansion: aa27b4f9-a959-4db4-bd3e-a0faa39c2ceb. UI implementation may
proceed while M0 integration tests compile; completion still requires shared M0
evidence. Verify focused component tests, TypeScript checks,
Rust checks and the isolated backend integration target; review the final diff.

## Current state

M0 and M1 are complete locally. Storage, collection, aggregation, authenticated
outcome updates and the Metrics display are implemented. The final isolated
backend run passed 39 integration tests, including second-page deployment
collection, redeployment idempotency, rejected credentials retaining evidence,
rolling-window boundaries, unresolved recovery and repository rename handling.
Both focused DORA unit tests and both structural checks passed. Frontend
typecheck, focused lint and 37 component/client tests passed. Library Clippy and
Rust formatting passed. Grouping the three metrics/job routes preserved their
paths and handlers while keeping the central router within its structural limit.

A broader Clippy invocation including all test code found an existing
`err_expect` lint in `backend/src/auth/tests.rs:485`; that unrelated file remains
unchanged. One intermediate REST run exposed a nonexistent-directory fixture
being marked absent by discovery; the fixture now creates a real temporary
directory and the final run passed. No live deployment or GitHub history was
queried. Implementation is ready for publication; deployed behavior remains
unverified.

## Scale correction — 2026-10-16

The original display and eager deployment payload did not fit the user's 50
repositories. Repair plan: 4f47a6a2-9d7a-4e30-865c-d1d600111dfd. Replace the
repository table and repeated disclosures with five portfolio metric tiles and
one paginated deployment view. The user clarified that metrics must be
aggregates; remove repository selection and repository metric panels. Preserve
production evidence, explicit outcomes and unknown values. Compute portfolio medians from
underlying samples and keep deployment detail reads outside metrics polling.
Verification uses a 50-repository PostgreSQL fixture with 60,000 deployments and
300,000 commits; live profiling is unavailable because the credential broker
refused database access. The user authorized committing and pushing this
correction on 2026-10-16. Deployed behavior remains unverified.
