# Retrieval API

Sulion exposes an agent-facing retrieval service at `SULION_RETRIEVAL_URL`.
PTYs receive `SULION_RETRIEVAL_TOKEN` automatically; agents should use
`sulion-retrieve` or `sulion retrieve ...` rather than hand-building requests.

The retrieval service reads canonical Sulion Postgres tables directly. It does
not maintain a duplicate transcript-text projection. Semantic search stores
embedding vectors plus source keys in `retrieval_embeddings`; result text is
loaded from `event_blocks` and timeline tables at query time.

## CLI

```sh
sulion-retrieve search "what did we decide about retrieval" --limit 5
sulion-retrieve search "exec_command" --include tools --include-low-value --tool-category utility
sulion-retrieve search "words the user said" --include user --mode lexical
sulion-retrieve file-history backend/src/retrieval/search.rs
sulion-retrieve turn <agent-session-uuid> <turn-id>
sulion-retrieve reindex --repo sulion
sulion-retrieve reset --confirm
sulion-retrieve index-status
sulion-retrieve facets
```

The CLI adds auth and context headers from the PTY environment:

- `SULION_REPO_NAME`
- `SULION_WORKSPACE_ID`
- `SULION_PTY_ID`
- current working directory

Repo scope is the default. Session scope is explicit:

```sh
sulion-retrieve search "exact thing" --scope session --session <agent-session-uuid>
```

Use `--json` when another tool needs stable machine-readable output.

Default search includes assistant text, not user instructions. Add
`--include user` when looking for what the user requested or decided, and use
`--mode lexical` when their wording is known. `facets` reports indexed scope;
`index-status` reports semantic backlog. A pending-index warning means
semantic results may lag even though lexical history is available.

## Turn lookup and file history

`sulion-retrieve turn <agent-session-uuid> <turn-id>` returns a compact markdown
digest: the prompt, assistant text, and one header per tool call. It does not
embed tool inputs, reconstructed diffs, or full results. Those remain in
canonical blocks and operation rows and can be inspected in timeline detail;
search returns per-turn operation and file-touch evidence.

`file-history <repo-relative-path>` lists turns that touched the path within
the selected repository. Collection-session access does not broaden retrieval
to all group members; choose the relevant repo explicitly when needed.

## Archived sessions

A session the archive loop has purged keeps only its turn digest (see
[`ingestion.md`](ingestion.md#retention-boundary)). Retrieval still answers
for it, at turn granularity:

- `search` returns `turn_digest` hits for purged sessions whenever the
  include set covers assistant, user, or summary text: lexical full-text over
  the turn markdown and prompt, semantic over one `turn_digest` embedding
  source per turn. Results and evidence packets carry `archived: true`;
  evidence lists the turn's files from the digest and no operations.
- `turn` returns the same markdown it always did, with `archived: true`.
- `file-history` unions per-touch rows of live sessions with the digest
  file lists of purged ones.
- Tool-scoped search (`--include tools`, `--tool-name`, `--tool-category`),
  operation detail, and tool output need the session restored:
  `sulion archive restore --session <uuid>`.

A `sulion-retrieve reset` rebuilds archived history too: the backfill
enumerates `turn_digest` sources alongside the three live families.

## Search

`GET /v1/search`

Required query:

- `q`

Common query parameters:

- `scope=repo|session|all`, default `repo`
- `repo`
- `agent_session_uuid`
- `include=assistant,user,summary,tool_call,tool_result,tool_error,tools`
- `include_low_value=true`
- `search_mode=hybrid|lexical|semantic`, default `hybrid`
- `tool_category`
- `tool_name`
- `file_path`
- `errors_only=true`
- `since`, `until`
- `limit`

Default `include` is `assistant`. Tool usage is excluded unless explicitly
requested with `include=...` or implicitly requested by passing
`tool_category` / `tool_name`. Low-value tool mechanics stay excluded even when
tools are included; pass `include_low_value=true` to search them as a single
coarse group. The low-value group covers shell/session mechanics
(`exec_command`, `bash`, `write_stdin`), edit payload mechanics
(`apply_patch`, `edit`, `write`), and inspection call mechanics
(`read`, `glob`, `grep`, `view_image`).

Results include:

- source identity (`source_kind`, `agent_session_uuid`, turn/offset fields)
- repo/session metadata
- snippet and preview text
- per-turn evidence (`operations`, `file_touches`)
- `tool` details for tool result kinds

## Semantic Indexing

`POST /v1/reindex`

```json
{
  "repo": "sulion",
  "agent_session_uuid": null
}
```

This starts durable cursor backfills for matching assistant/user/summary text
and tool call/result sources. It does not scan transcripts or call the
embedding service in the request path. The background retrieval indexer advances
those backfills in keyset batches, marks changed or missing source keys pending,
and drains pending sources through the local embedding service at
`SULION_RETRIEVAL_EMBEDDING_URL`, defaulting to
`http://192.168.66.3:5361`.

On startup, if the semantic source-state tables are empty, the retrieval service
automatically schedules the initial repo-wide cursor backfills. The worker runs
continuously while backfills or pending sources exist, then falls back to the
idle interval configured by `SULION_RETRIEVAL_INDEX_SECONDS` (default 300s).

`GET /v1/index/status` reports pending, indexed, failed, and deleted semantic
source counts, running/failed backfill counts, cumulative backfill rows seen,
and the embedding count for the active model.

`POST /v1/index/reset` (body `{"confirm": true, "reschedule": true}`) drops all
embeddings and the backfill/source queue, then reschedules a full rebuild under
the current source-selection and chunking rules. It is the supported way to
rebuild from scratch: it runs under the same indexer lock as the background
crawler, so it never truncates tables out from under an in-flight backfill, and
the service — not a raw SQL wipe — orchestrates the restart. `confirm` is
required; pass `"reschedule": false` to wipe without scheduling a rebuild.
Transcript text is untouched; only the derived embedding state is reset. The
embedding service should be capped/idle before a large rebuild.

## Chunking

Long sources (assistant/user/summary text and `agent` subagent finals) are split
into `SULION_RETRIEVAL_EMBEDDING_MAX_CHARS`-sized chunks, up to
`SULION_RETRIEVAL_EMBEDDING_CHUNK_MAX` (default 10) per source; the tail beyond
the cap is dropped. Each chunk is one row in `retrieval_embeddings`, keyed by
`(embedding_model, source_key, chunk_ord)`; `retrieval_embedding_sources` stays
one row per source, so index status and source counts remain source-based while
`embedding_count` counts chunks. Search collapses multiple chunk hits of one
source to its best-scoring chunk.

Most tool output is not embedded: only natural-language reasoning and short
intent are indexed. `tool_call` keeps the tool name plus a capped `input`
(intent, not the file body/diff), `tool_error` keeps a capped message, and among
results only `agent` finals are embedded in full. Command output, file reads,
writes, diffs, image payloads, and MCP state dumps are dropped — they are
redundant with the code index and dilute semantic search.

The current local model is `nomic-ai/nomic-embed-text-v1.5` with 768 dimensions.
Vectors live only in `embedding_vector vector(768)` under an HNSW index; both
are created by migration, and the service refuses to start if the column's
dimension does not match the configured model or the index is invalid. There
is no `REAL[]` copy and no exact-scan fallback: pgvector is required.

Embedding writes are batched, and timeline reprojection preserves unchanged
operation source hashes and embeddings. Only changed or removed sources need
reconciliation.

The browser UI exposes the same backfill scheduling path from the sidebar admin controls.
That action calls the authenticated backend endpoint
`POST /api/admin/retrieval/reindex`; the backend proxies the scheduling
request once with `SULION_RETRIEVAL_TOKEN`, so the static retrieval token is
never sent to the browser.
