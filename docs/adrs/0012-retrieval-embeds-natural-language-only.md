# ADR 0012: Semantic Retrieval Embeds Chunked Natural-Language Content Only

## Status

Accepted, 2026-06-22.

## Context

The retrieval service answers agent queries over transcript history with a
lexical path over `event_blocks` and a semantic path through a local
`nomic-embed-text-v1.5` server on TrueNAS. Vector storage is covered by
ADR 0003; this record is about what gets embedded.

The first semantic indexer embedded every source kind — assistant and user
text, summaries, every tool call, every tool result — as one embedding per
source, truncated. It did not run for long. On 2026-06-15 it was stalled
outright: batches of 64 against a server that caps client batches at 32
returned 422 (6821d28), and lexical search seq-scanned `event_blocks` at 49 s
a query (a30b20d). Once unstalled it saturated all 72 cores of the TrueNAS
host: about 265k sources pending, draining at roughly 150 embeddings a
minute, with backfills enumerating faster than the embedder drained
(1c2bb4f, 2026-06-16). The worker was disabled by setting the idle interval
to zero while the shape was reconsidered.

Measuring the longest sources settled the shape. Nearly everything over 60k
characters was not prose: `write` and `apply_patch` tool calls carrying whole
files and diffs, MCP state dumps, GitHub-issue JSON, and truncation notices.
The long content worth keeping was subagent finals (about 77k) and long
assistant messages (about 43k). The code-intelligence index (ADR 0001)
already holds source structure, so embedding file bodies and diffs indexed
the same material twice.

## Decision

Semantic indexing embeds natural-language reasoning and short intent only:

- assistant, user, and summary text, and `agent` subagent finals, split into
  chunks of `SULION_RETRIEVAL_EMBEDDING_MAX_CHARS` up to ten per source;
- `tool_call` as tool name plus a capped `input` — the intent, not the file
  body or diff;
- `tool_error` as a capped message.

Command output, file reads and writes, diffs, image payloads, and MCP state
dumps are not embedded. Low-value tool mechanics (shell, edit, read, grep)
are also excluded from search results by default behind
`--include-low-value`.

`retrieval_embeddings` gains `chunk_ord`, keyed by `(embedding_model,
source_key, chunk_ord)`; `retrieval_embedding_sources` stays one row per
source so status and counts remain source-based. Search collapses the chunks
of one source to its best-scoring chunk. Migration 0053 is non-destructive:
existing rows become chunk 0.

`POST /v1/index/reset` (`sulion-retrieve reset --confirm`) rebuilds the index
under the current rules. It runs under the indexer lock so it cannot truncate
tables beneath an in-flight backfill, then reschedules a full backfill.
Transcript text is untouched.

## Alternatives considered

- **One truncated embedding per source, for every source kind.** The prior
  state. Rejected on measurement: 265k pending at 150 a minute with 72 cores
  pegged (1c2bb4f), and truncation discarded the tail of exactly the long
  prose worth finding.
- **Keep every tool call embedded in full.** Proposed mid-design after the
  result filter was settled, then withdrawn in the same session once the
  longest kept sources turned out to be `write` and `apply_patch` inputs —
  whole files and diffs, "the exact same redundant-code problem as
  exec_command" (session `d71cf620-2c05-4ecb-bffd-4b41cbf673f8`, turns 881411
  and 1101916). 677bb6c: "they duplicate the code index and dilute semantic
  search."
- **Skip chunking, since little kept prose exceeds one chunk.** Raised when
  the filter removed most multi-chunk sources. The user kept it: "we should
  still do chunking. it's a value add only. it just wont be used very often"
  (turn 881411). The cap of ten chunks bounds the rare long source.
- **A second code index built from embedded file bodies.** Rejected because
  code intelligence already indexes repositories and workspaces, and the
  retrieval service deliberately reads canonical tables without duplicating
  transcript text (`architecture.md`, Retrieval Service).
- **A raw SQL wipe to restart the index.** Rejected in favour of the reset
  route, so the service rather than an operator orchestrates the restart and
  the indexer lock protects an in-flight backfill (677bb6c, `retrieval.md`).

## Consequences

- Embedding volume roughly halved (677bb6c, CHANGELOG v1.6.0), and the
  indexer can run continuously again instead of being switched off.
- Semantic search answers "what was reasoned or decided", not "which command
  printed this". Tool output is reachable lexically or with `--include tools`.
- Index status counts sources; `embedding_count` counts chunks.
- Any change to the selection rules needs `sulion-retrieve reset --confirm`
  and a full re-backfill, with the embedding server idle beforehand.
- ADR 0003 later removed the `REAL[]` copy, required pgvector, and added one
  `turn_digest` source per archived turn under the same chunking rules.

## References

- Chunking and source selection: [`../retrieval.md`](../retrieval.md#chunking)
- Retrieval service boundary: [`../architecture.md`](../architecture.md#retrieval-service)
- Code intelligence as the source index: [ADR 0001](0001-code-intelligence-agent-tool.md)
- Vector storage and archived sources: [ADR 0003](0003-tiered-transcript-retention.md)
- CHANGELOG v1.6.0, "Retrieval"
- Commits: 6821d28 (batch split), a30b20d (trigram index), 1c2bb4f (indexer
  disabled, measured saturation), 677bb6c (chunking, filter, reset), 8a4897c
- Transcript: session `d71cf620-2c05-4ecb-bffd-4b41cbf673f8`, turns 266987,
  793470, 881411, 1101916
