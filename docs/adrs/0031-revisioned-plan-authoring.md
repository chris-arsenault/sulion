# ADR 0031 — Revisioned plan authoring

## Context

Related changes from a planning turn must reach the next agent together.
Compact progress surfaces need a short summary. Handoff requires the full
intent, phase implementation, and evidence, including superseded decisions.
Terminal, browser, and scoped MCP clients edit the same plan.

## Decision

Keep the authored document on the existing plan row. Use stable IDs for its
entries and the existing relational phases. Store implementation and evidence
by phase UUID. A typed patch checks the authoring revision under the plan row
lock and commits the document, compact guidance and phase projections, and
before/after history together. Keep the operational revision for runtime
updates. Load authored details on a selected plan read and derive stable entry
IDs for legacy guidance until the first native patch materializes the document.

Use the existing plan service as the transaction boundary and adapt each
transport to it. The MCP SDK owns Streamable HTTP and tool discovery. The
connector authenticates a dedicated resource audience, client and subject and
scopes its tools to explicitly connected repositories. The existing branch
model continues to own execution hierarchy and live inherited guidance.

This amends ADR 0014's decision to keep detailed planning outside Sulion.
Retain its compact progress projection and branch ownership. A Markdown
sidecar would require every client to resolve and write files as well as
coordinate revisions with the plan. A separate relational table for each
kind of entry would add joins and migrations to a selected-plan read. The
document shares the existing plan's ownership and transaction boundary.

## Consequences

Concurrent editors rebase against a current read and its history. A change in
intent can retire an entry and link successors without rewriting historical
evidence. Earlier revisions are recoverable from change history. Pending work
has a compact projection, and retired scope remains distinguishable from
delivered phases. Scope queries that later need to range across authored
entries may merit a relational projection. Current authoring and handoff load
one plan and its ancestors at a time.
