# Code Intelligence

Sulion code intelligence is an agent-facing structural navigation contract for
source repos and workspaces. It is independent of transcript retrieval and has
no browser UI surface.

Status: this document defines the canonical contract. The service skeleton,
parser, indexer, real CLI, and `/v1/help`, `/v1/status`, `/v1/refresh`,
`/v1/outline`, `/v1/find`, `/v1/def`, `/v1/refs`, `/v1/search`, `/v1/patch`,
and `/v1/pack` service routes exist. Semantic `def`/`refs` escalation is
attempted when a configured language server is available.

Durable design decisions live in
[`adrs/0001-code-intelligence-agent-tool.md`](adrs/0001-code-intelligence-agent-tool.md).

## Agent Entry Point

Agents should learn one command:

```sh
sulion-code help
```

`sulion-code` is the documented PTY-facing command. Internally it may delegate
to `/usr/local/bin/sulion code ...`, but agent instructions should use
`sulion-code`.

Scope is inferred from the current working directory and Sulion PTY
environment. To query a different repo or workspace, change directories first.

## Global Options

Only two global options are part of the canonical interface:

```sh
--json
--budget small|normal|large
```

- `--json` returns stable machine-readable output with `schema_version`.
- `--budget` controls output size. `normal` is the default.

Avoid adding repo, workspace, precision, or backend selector flags to the
canonical path. The service owns those choices.

## Commands

```sh
sulion-code help
sulion-code status
sulion-code refresh [path]
sulion-code outline [path]
sulion-code find <symbol-or-name>
sulion-code def <path:line[:col] | symbol-id>
sulion-code refs <path:line[:col] | symbol-id>
sulion-code search <lang> <pattern> [path]
sulion-code patch <lang> <pattern> <rewrite> [path]
sulion-code pack <path:line-line | symbol-id>
```

### help

Prints the concise command reference agents should read at the start of a new
session.

Example:

```sh
sulion-code help
```

### status

Shows the inferred root, index freshness, supported languages, semantic
availability, language-server health, timeout behavior, fallback behavior, and
three useful next commands.

Example:

```sh
sulion-code status
```

### refresh

Refreshes the current root or a path within it.

Examples:

```sh
sulion-code refresh
sulion-code refresh backend/src/api
```

### outline

Returns a compact structural outline for a file or directory. Bodies are elided
unless needed for a budgeted context pack.

Examples:

```sh
sulion-code outline backend/src/main.rs
sulion-code outline backend/src/api --budget small
```

### find

Finds matching symbols by name. The response is ranked and range-addressable.

Example:

```sh
sulion-code find RetrievalState
```

### def

Finds the definition for a file position or symbol id. The service attempts
semantic resolution where available and falls back to syntactic index results.

Examples:

```sh
sulion-code def backend/src/retrieval.rs:145
sulion-code def sym_01J...
```

### refs

Finds references for a file position or symbol id. Results carry confidence so
agents can tell semantic references from syntax-based approximations.

Example:

```sh
sulion-code refs backend/src/retrieval.rs:145:12
```

### search

Runs structural search with an ast-grep pattern.

Examples:

```sh
sulion-code search rust 'impl $TYPE { $$$ }' backend/src
sulion-code search tsx '<$COMP $$$ />' frontend/src
```

### patch

Runs a structural rewrite and returns a unified diff. It does not apply the
diff.

Example:

```sh
sulion-code patch rust 'foo($A)' 'bar($A)' backend/src
```

### pack

Returns a token-budgeted context bundle for a symbol or range.
The bundle includes the primary signature/range, containing symbols, important
imports, a clipped source excerpt, and useful reference or test-reference rows
when available.

Examples:

```sh
sulion-code pack backend/src/retrieval.rs:130-230
sulion-code pack sym_01J... --budget large
```

## Text Output Contract

Default text output is for agents deciding what to inspect next. It should be
compact, deterministic, and range-addressable.

Every result should include:

- file path
- start and end range
- symbol or match kind
- name or short label
- confidence
- freshness
- truncation or parse warnings when present

Example shape:

```text
backend/src/retrieval.rs:136-260 syntactic fresh struct RetrievalState
  impl methods: from_config, from_pool_for_tests, refresh_vector_capabilities
  next: sulion-code pack backend/src/retrieval.rs:136-260
```

## JSON Output Contract

JSON output uses stable snake_case fields and includes `schema_version`.

Common envelope:

```json
{
  "schema_version": 1,
  "command": "outline",
  "root": {
    "kind": "workspace",
    "name": "sulion",
    "path": "/home/dev/workspaces/sulion/example"
  },
  "freshness": "fresh",
  "confidence": "syntactic",
  "warnings": [],
  "results": []
}
```

Range shape:

```json
{
  "path": "backend/src/retrieval.rs",
  "start_line": 136,
  "start_col": 1,
  "end_line": 260,
  "end_col": 2
}
```

Symbol result shape:

```json
{
  "id": "sym_01J...",
  "kind": "struct",
  "name": "RetrievalState",
  "qualified_name": "retrieval::RetrievalState",
  "signature": "pub struct RetrievalState",
  "range": {
    "path": "backend/src/retrieval.rs",
    "start_line": 136,
    "start_col": 1,
    "end_line": 260,
    "end_col": 2
  },
  "confidence": "syntactic",
  "freshness": "fresh"
}
```

Patch result shape:

```json
{
  "schema_version": 1,
  "command": "patch",
  "matches": 2,
  "applied": false,
  "diff": "--- a/backend/src/example.rs\n+++ b/backend/src/example.rs\n..."
}
```

## Confidence

Every answer reports confidence:

- `semantic`: language-server-backed result.
- `syntactic`: Tree-sitter/index-backed result.
- `mixed`: semantic result with syntactic fallback rows.
- `stale`: answered from an index known to be out of date.
- `partial`: parse errors, unsupported files, or budget truncation affected the
  answer.

Syntactic results must not be presented as semantic certainty.

## Troubleshooting

- If `sulion-code` is not found, the current deployment does not include the
  code-intelligence CLI yet.
- If `sulion-code` reports `SULION_CODE_INTEL_URL` or
  `SULION_CODE_INTEL_TOKEN` is missing, the shell is not running with Sulion PTY
  code-intelligence environment forwarding.
- If `status` reports a stale index, run `sulion-code refresh` before depending
  on symbol or reference results.
- If a file has parse errors, `outline`, `find`, and `search` may still return
  partial results; check the confidence and warnings fields.
- If `def` or `refs` returns syntactic confidence, check `status.semantic` for
  the missing server, startup, health, timeout, or fallback reason.
