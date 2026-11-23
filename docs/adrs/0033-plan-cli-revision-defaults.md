# ADR 0033 — Terminal plan edits use the current revision by default

## Context

The Rolling Local handoff required repeated CLI reads and schema inspection.
Routine edits used two revision counters, UUIDs, and different revision
requirements depending on the command.

## Decision

CLI edits apply to the current state under the existing plan row lock.
Resolve phase numbers inside the same transaction and store stable phase IDs.
An explicit revision still checks the exact version on which an edit depends.
Batch writes commit together. The REST patch contract requires an authoring revision.
CLI and MCP authors can submit ordinary edit commands in order. The service resolves
numbered references as it applies each command under the plan row lock.

Fetching a revision just before every write would add a request and create
conflicts with unrelated edits between the read and write. Caching the last
read would make command behavior depend on terminal history. Field-scoped
updates under the lock keep the current plan and its history as the authority.

## Consequences

Two edits to distinct fields can both apply. A later edit to the same field
uses the latest supplied value unless the caller requests an explicit revision
comparison. A new evidence record defaults to the plan and ancestor versions
at submission. A caller recording an older observation can supply the
versions it checked. This amends
the terminal write behavior described in [ADR 0031](0031-revisioned-plan-authoring.md).
