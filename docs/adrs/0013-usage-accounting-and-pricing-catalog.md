# ADR 0013: Usage Counted Once per Message with a Priced Model Catalog Carrying Provenance

## Status

Accepted, 2026-10-09.

## Context

The Overview and Metrics tabs show token spend from `agent_session_usage`,
which the ingester fills from Claude and Codex transcripts. On 2026-07-23 the
user saw about 500M tokens for single sessions and suspected either
misinterpretation by an order of magnitude or multi-counting (session
`b65c0716-9a73-4760-a146-57361444d369`, turn 13365010). One transcript
settled it: 421 assistant lines but 208 distinct API responses, because
Claude Code writes one JSONL line per content block and repeats the same
`message.usage` on each. Summing lines gave 91.2M; counting once per message
gave 46.3M, a 1.97x inflation that grows with tool-heavy sessions. Of the
honest number, 97.6% was cache reads.

The first repair collapsed the display into "fresh" and "processed". By
August that was the problem: the large tile never said what it was, output
was invisible, and the user could not reconcile input, output, and cached
(session `01a01708-4444-74c3-a1ca-e3df9336c29b`, turn 92898). Commit 89266ac
split the categories and added a priced catalog.

On 2026-10-09 the Metrics pane showed `claude-opus-5-5`, `gpt-6.1-sol`, and
`fugu` as "price unavailable", with the $559.01 total covering Astra only
(session `01a121b1-7022-71f0-b661-8a433905b431`, turn 103172). Plain Fugu
bills by the underlying models it routes to, so it has no published rate.

## Decision

Usage counts once per message. For Claude, the latest receipt for a message
id replaces any earlier contribution, including downward corrections and
changes of day or model; an indexed lookup makes that survive restarts and
interleaving. For Codex, deduplicated `token_usage_record` responses are the
source, including compaction; response ids persist in
`agent_usage_responses`, and a session keeps its legacy cumulative prefix
until the first valid response record. Both compose with the
`(session_uuid, byte_offset)` idempotency key.

Categories stay separate end to end: standard input, cache writes (ordinary
and one-hour retained separately so provider rates apply without inference),
cache reads, and output. `cached_input_tokens` uniformly means cache reads.
Rollups report input, cached input, and output; input includes cache writes
and excludes cache reads. Daily attribution lives in
`agent_model_usage_daily`. The usage projection is versioned and rebuilt from
stored events at control startup; archived rollups are preserved (ADR 0003).

Pricing is a catalog in `metrics.rs` of per-model standard-tier API list
rates applied to all dates. Estimates are API-equivalent spend; they do not
claim to reproduce subscription charges or historical invoices. A model
without a catalog entry shows as unpriced and marks totals incomplete; it is
never silently zero. Each price carries a tagged `basis`: `published`, or
`estimated` with `reference_model`, `source_url`, and a `note`. Plain `fugu`
is priced as an estimate from Fugu Ultra v2.0 and the pane shows the
reference and explanation.

## Alternatives considered

- **Sum every assistant JSONL line.** The prior ingester. Rejected on the
  1.97x measurement in db4a22f and turn 13365010.
- **One cost-weighted number that tracks subscription burn.** Offered at turn
  13365010 as the alternative to a token headline and described there as
  "pricing-assumption-laden". Rejected; the CHANGELOG entry for 89266ac
  states estimates "do not claim to reproduce subscription charges or
  historical invoices". Separate categories plus an explicit API-equivalent
  estimate were chosen instead.
- **"Fresh versus processed" as the headline.** The July–August display.
  Rejected after turn 92898 showed it hid the input/output split and read as
  ambiguous; 89266ac replaced it with input, cached input, and output and
  per-model rates.
- **Price unknown models at zero.** Rejected in 89266ac ("Unknown model
  prices remain visibly unpriced", `docs/plans.md`). The visible gap is what
  surfaced three missing catalog entries on 2026-10-09; a silent zero would
  have understated spend without a signal.
- **Leave Fugu unpriced because it has no fixed rate.** Rejected by the user,
  who authorized an explicitly labelled proxy (plan
  `1e720c38-5f88-4946-ac08-76b89076314b`, principle: "Fugu uses an explicitly
  labelled proxy authorized by the user"). The provenance tag keeps it from
  passing as a published rate.

## Consequences

- Startup rebuilds the usage projection when its version changes; the
  projection tables stay locked until replacement completes, and admin
  reindex rebuilds usage too.
- Adding a catalog entry re-prices all history at once. Totals previously
  marked incomplete become priced; there is no per-date rate history.
- Estimated rates are approximate by construction. Plain Fugu's orchestration
  may bill differently from the proxy; the UI says so beside the number.
- The full production inventory of unpriced models never ran: plan
  `1e720c38` closed on 2026-10-27 with that phase skipped because the broker
  injected no database credential. Coverage beyond the three added models is
  unverified, and the inventory is a backlog item.

## References

- Usage accounting: [`../ingestion.md`](../ingestion.md#usage-accounting)
- Metrics categories and runtime boundary: [`../plans.md`](../plans.md)
- Retention of archived rollups: [ADR 0003](0003-tiered-transcript-retention.md)
- CHANGELOG Unreleased: "Metrics and background work" and "Model pricing"
- Commits: db4a22f (dedup by message id), 89266ac (categories and pricing),
  faaeb5a (missing prices, estimated Fugu rate)
- Published plan: `1e720c38-5f88-4946-ac08-76b89076314b`
- Transcript: `b65c0716-9a73-4760-a146-57361444d369` turn 13365010,
  `01a01708-4444-74c3-a1ca-e3df9336c29b` turn 92898,
  `01a121b1-7022-71f0-b661-8a433905b431` turn 103172
