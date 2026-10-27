# ADR 0023: The Stored Turn Digest Holds the Prompt, Assistant Text, and One Header per Tool Call

## Status

Accepted, 2026-09-20.

## Context

`timeline_turns.markdown` is the turn digest: the text `sulion-retrieve
turn` returns, "copy turn as markdown" copies, the archive purge keeps when
a session's events are deleted (ADR 0003), and one embedding source per
turn is built from. Until this decision the digest embedded every tool
call's input, a diff reconstructed from edit calls, and the full result
body.

All of that already lived elsewhere. Operation rows in
`timeline_operations` and canonical `event_blocks` hold the inputs and
results that turn detail renders and that search indexes. The digest was a
fourth copy, and because the ingester re-upserts a live turn's row on every
tick that appends to it, the copy was regenerated and rewritten each time
the turn grew. Measured on 2026-09-20, the average digest was 48 kB and a
long turn's was over a megabyte.

The same row also carried `turn_json`, a whole-turn JSON copy that nothing
read: the timeline API served `chunks_json` and joined operation rows,
retrieval read `markdown`, and every rebuild derived from `events.payload`.
At 2 MB for a long Codex turn it was rewritten per event beside the
regenerated markdown, and was the largest write-ahead-log source measured
in the system.

## Decision

The digest carries the prompt, the assistant's text blocks, and one header
line per tool call. Tool inputs, reconstructed diffs, and result bodies are
not part of it. A reader that needs them opens timeline tool detail or
queries operation rows; search still returns per-turn operation and
file-touch evidence from the canonical tables.

Timeline projection v10 re-renders stored digests that still hold fences or
result labels, so retained history converges on the next control start
without a reindex. Archived digests written by the purge before this change
are re-rendered the same way, since the purge stores the same markdown the
live path composes.

Migration 0087 drops `turn_json`. A session that keeps growing is projected
at most once every 5 seconds; a session that stops growing is projected on
the next tick, so a finished turn still appears within a tick. The interval
is a config value that defaults to zero for tests; the ingester and
standalone binaries pass the 5-second constant.

The digest's contract is therefore what a person needs to recognise a turn
and what a search needs to find it: what was asked, what the assistant
said, and which tools ran. It is not a serialisation of the turn.

## Alternatives considered

- **Keep embedding tool inputs, diffs, and results in the digest.** The
  prior shape. Rejected on measurement: the commit body for `c3fe6a5`
  records the 48 kB average and megabyte-plus long turns, rewritten on each
  growth, for content that `timeline_operations` and `event_blocks` already
  served to turn detail and search. The only consumer that lost content is
  a reader of the raw digest, and that reader has the operation rows.
- **Keep a whole-turn JSON column beside the markdown.** `turn_json` was
  the standing version of this. The commit body for `94056c8` records that
  no code path read it and that it was the largest WAL source measured;
  retaining it would have meant keeping a 2 MB-per-event rewrite for an
  export format with no reader. Rebuilds derive from `events.payload`, so
  nothing was lost by dropping it.
- **Append to the stored digest instead of regenerating it.** Considered
  during the later incremental-projection work and rejected there for the
  same reason it would not have helped here: appending to a growing text
  value still rewrites the whole row. The durable fix for live turns was
  composing the digest on read (ADR 0024); this decision fixed what the
  digest contains.

## Consequences

- `sulion-retrieve turn` and "copy turn as markdown" return a shorter
  document. A reader who wants a tool's input or output follows the
  operation reference; retrieval evidence packets list operations and files
  beside the digest hit.
- Archived search is unchanged in kind but smaller in body: a purged
  session's turn is found by its prompt and assistant text, not by text
  that appeared only inside a tool result. ADR 0003 already stated that
  tool output needs a restore.
- Live turns appear in the timeline up to 5 seconds later than before
  while they are still growing; finished turns are not delayed.
- The digest is now stable enough to be the purge's retained artefact
  without the purge paying for a megabyte per long turn, which is what
  made ADR 0003's turn-grained tier affordable.
- Rendering code must keep the two outputs distinct: the digest renderer
  emits headers, the turn-detail renderer emits bodies. A change that puts
  bodies back into `markdown` reintroduces the rewrite cost.

## References

- Digest contract: [`../ingestion.md`](../ingestion.md), "Live projection"
- Retrieval behaviour: [`../retrieval.md`](../retrieval.md), "Turn lookup
  and file history" and "Archived sessions"
- Retention tier that stores the digest:
  [`0003-tiered-transcript-retention.md`](0003-tiered-transcript-retention.md)
- Follow-on that composes live digests on read:
  [`0024-incremental-per-event-timeline-projection.md`](0024-incremental-per-event-timeline-projection.md)
- Change record: `CHANGELOG.md`, Unreleased, "Changed the turn digest" and
  "Fixed a long-running turn rewriting megabytes per event"
- Commits: `c3fe6a5` — Reduce the turn digest to prompt, assistant text,
  and tool headers; `94056c8` — Drop the unread turn_json column and
  debounce live-turn projection
