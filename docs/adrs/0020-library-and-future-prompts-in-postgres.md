# ADR 0020: Library and Future Prompts in Postgres

## Status

Accepted, 2026-08-21.

## Context

The prompts and references library began as per-repository markdown files
under `<repo>/.shuttlecraft/refs/` and `/prompts/` (`101988b`, 2026-04-19):
no schema, no database, git-committable, and trivially surviving backend
restarts while the backend was one process on one machine. `60b7df5` made
the library global, and the control API later read it from a configured
`library_root` on the local filesystem, by default under the service home.
Future prompts, the per-session queue of deferred follow-ups, were stored
the same way.

After the control-plane and node split (ADR 0002), the library routes still
read `state.library_root` in whichever process served them, with no node
proxying. The control-plane container on TrueNAS answers `/api/library`
and has no library mount, so it listed an empty ephemeral directory. The
user reported that saved prompts and references had disappeared; the files
were intact on the node, unreachable by the API. Future prompts had the same
shape and the same exposure.

Offered a choice between proxying library reads over the node channel and
moving the data into Postgres, the user answered that these were expected to
be database-backed.

## Decision

Reusable prompts and saved references live in `library_entries`, keyed by
`(kind, slug)`, served by the control API from Postgres (migration 0079).
Future prompts live in `future_prompts`, keyed by `(session_uuid, id)`, with
a `pending` or `sent` state; `future_prompt_session_state` carries the
pending count and revision for app-state reads (migration 0080). Neither
requires a node-local directory, and both are available to the API in every
deployment topology.

Future prompts are one-off follow-ups bound to a transcript session and leave
a PTY's queue when that session is no longer current. Injection from either
store targets the timeline composer in effective timeline-only mode and the
terminal otherwise (ADR 0019); injection into the composer does not submit.

Existing markdown entries were imported once by generated SQL after the
deploy created the tables.

## Alternatives considered

- **Keep node-local markdown directories.** The prior design, chosen when
  one process owned both the API and the filesystem. Under the split
  topology the control API cannot see node-local files, and the deploy that
  moved the backend to TrueNAS silently emptied the library without deleting
  anything. The property that made markdown attractive, user-editable
  git-committable files, had already been given up when the library went
  global and moved out of the repository tree.
- **Proxy library and future-prompt reads over the node protocol.** The fix
  recommended first in the diagnosing session, keeping the node's home as
  the single storage location like the repo and file routes. The user
  rejected it in favor of Postgres: the entries are application data, not
  repository content, and a node-proxied file store would still disappear
  whenever the node was offline and would add typed node operations for
  data that has no reason to live on the development host.

## Consequences

- The library and future-prompt queue survive control restarts, node
  outages, and topology changes, and the e2e stack seeds them through the
  database like other application data.
- Entries are no longer files a user can edit or commit; editing happens
  through the UI and API.
- Future-prompt state moved from a file-backed per-PTY structure to rows
  keyed by transcript session, so a resumed session in another PTY carries
  its queue with it.
- `library_root` and its filesystem code path are gone from the control
  process; nothing in the control plane reads a node-local path for these
  features.

## References

- Current contract:
  [`../architecture.md`](../architecture.md#library-and-future-prompts)
- Topology that made the move necessary:
  [ADR 0002](0002-hybrid-control-plane-and-dev-node.md)
- Injection target rule:
  [ADR 0019](0019-display-modes-and-tabs-rendered-by-reference.md)
- Commits: `101988b` and `60b7df5` (markdown library), `2e75923`
  (Postgres migration and route rewrite)
- Transcript: session `900989d9-0915-428f-ad97-701295c4968a`, turns
  `15927758` (diagnosis and proxy recommendation), `16068603` (user:
  expected database-backed)
- CHANGELOG Unreleased, "Library and secrets"
