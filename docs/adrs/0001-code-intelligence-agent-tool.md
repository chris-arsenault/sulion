# ADR 0001: Agent-Facing Code Intelligence

## Status

Accepted, 2026-06-01.

## Context

Sulion agents need a canonical way to navigate source structure from inside a
PTY. The tool is not a browser feature and is not transcript retrieval. New
agent sessions will not have this project-specific command in training data, so
the entry point has to be discoverable from a short command reference.

The code-intelligence service also has to work over mounted repos and
workspaces without duplicating source files or storing full ASTs in Postgres.

## Decision

Sulion ships a standalone `code-intel` service and an agent-facing PTY command:

```sh
sulion-code help
```

The documented command is `sulion-code`. It delegates internally to
`sulion code ...`, but agent guidance should not teach the internal dispatch.

The service boundary is a static bearer-auth HTTP API under `/v1/*`. PTYs
receive `SULION_CODE_INTEL_URL` and `SULION_CODE_INTEL_TOKEN`; the CLI adds the
auth and Sulion context headers. Scope is inferred from cwd and PTY metadata.
Agents change directories to change scope.

The durable index stores compact facts only:

- roots, files, hashes, languages, parse/index status
- symbols, deterministic symbol ids, ranges, signatures, parent links
- lightweight syntactic references and imports
- index job state and freshness metadata

The index does not store full source text or serialized AST blobs. Source text
is read from the mounted filesystem when a command needs snippets, structural
matches, diffs, or context packs.

The implementation uses three cooperating tiers:

- Tree-sitter for always-on parsing and syntactic symbol extraction
- ast-grep for structural `search` and diff-only `patch`
- persistent language-server clients for semantic `def` and `refs` resolution

Confidence labels are part of the contract: `semantic`, `syntactic`, `mixed`,
`stale`, and `partial`. The CLI and API must not present syntactic fallback as
semantic certainty.

`patch` returns a unified diff only. The first version does not apply edits.

## Consequences

The agent surface is intentionally small:

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

Only two global options are canonical: `--json` and
`--budget small|normal|large`.

Postgres remains a compact fact index, not a source-code projection. Query-time
filesystem reads are acceptable because repos and workspaces are already mounted
into the service container read-only.

Semantic navigation is a hard requirement for the languages the service
advertises as semantic: Rust, TypeScript, TSX, and JavaScript. The service keeps
persistent language-server clients for recently active roots, not every known
checkout. Rust uses one rust-analyzer per active root; TypeScript, TSX,
JavaScript, and JSX share one TypeScript-family server per active root. Servers
start lazily, allow a longer first-use warmup for workspace loading, reuse the
normal timeout for warmed request round trips, expire after an idle timeout, and
are bounded by a global active-server cap. `sulion-code status` surfaces full
runtime dependency detection, per-language health, active warmed roots, active
server count, cap, idle timeout, warmup timeout, request timeout, and explicit
fallback behavior so agents know when an answer is syntactic instead of
semantic.

## References

- Agent contract: [`../code-intel.md`](../code-intel.md)
- Deployment shape: [`../deploy.md`](../deploy.md)
