# ADR 0032 — Arconis references in Sulion plans

## Context

A ChatGPT conversation can author both a diagram and a plan. It needs to link
individual objects to phases, attach the diagram to the plan, and make the
same workspace discoverable to subsequent plans for the repository.

## Decision

Arconis owns the encrypted saved workspace and its room and object IDs.
Sulion stores the workspace UUID, room ID and title in a repository registry.
The registry outlives any single plan. Plan attachments and their phase-to-object
links live on the revisioned plan document and are written by the existing plan
patch service. That service checks the repository and phase identities when
it writes a link. The selected-plan read resolves registry details for display.

Sulion exposes phase URLs keyed by plan and phase UUID. Arconis uses its
existing external hyperlinks for phase URLs. The plan assembles Arconis object
deep links from the room and object IDs. The assistant writes both sides using
the two authenticated connectors. The plan stores references without needing
Arconis credentials or a copy of the canvas.

## Consequences

Retitling phases and renaming repositories preserve links. Each plan can attach
the same diagram with different phase links. Link updates across the two apps
are separate saves. An interrupted edit can be finished by reading the workspace
and the plan, then completing the missing change. Opening an attached diagram
or workspace still follows Arconis access rules.
