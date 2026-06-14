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
sulion-retrieve search "exec_command" --tools --tool-category utility
sulion-retrieve file-history backend/src/retrieval/search.rs
sulion-retrieve turn <agent-session-uuid> <turn-id>
sulion-retrieve reindex --repo sulion
sulion-retrieve index-status
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

## Search

`GET /v1/search`

Required query:

- `q`

Common query parameters:

- `scope=repo|session|all`, default `repo`
- `repo`
- `agent_session_uuid`
- `include=assistant,user,summary,tool_call,tool_result,tool_error,tools`
- `search_mode=hybrid|lexical|semantic`, default `hybrid`
- `tool_category`
- `tool_name`
- `file_path`
- `errors_only=true`
- `since`, `until`
- `limit`

Default `include` is `assistant`. Tool usage is excluded unless explicitly
requested with `include=...` or implicitly requested by passing
`tool_category` / `tool_name`.

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

The current local model is `nomic-ai/nomic-embed-text-v1.5` with 768 dimensions.
When `pgvector` is installed in the `sulion` database, the retrieval service
creates `embedding_vector vector(768)` and a non-blocking HNSW index. Without
pgvector, semantic search exact-scans the stored `REAL[]` vectors.

The browser UI exposes the same backfill scheduling path from the sidebar admin controls.
That action calls the authenticated backend endpoint
`POST /api/admin/retrieval/reindex`; the backend proxies the scheduling
request once with `SULION_RETRIEVAL_TOKEN`, so the static retrieval token is
never sent to the browser.
