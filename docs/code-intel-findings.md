# Code-intel investigation — findings & handoff

Status of the `sulion-code-intel` service after the latest deploy, written for a fresh
agent. Live state was observed via `sulion-code status` / `--json status` and one line
of container logs the user pasted. Claims are marked **verified** vs **inferred**.

## TL;DR

Three deploy fixes shipped in commit `aa4eb59` and **work**. Two real problems remain:

1. **The indexer hangs** on the tree-sitter parse of one file (no parse timeout). This is
   the show-stopper — the index never drains (0 of 274 files parsed) and `sulion-code
   refresh` blocks forever. Fix is in `parser.rs`. **Not yet implemented.**
2. **The LSP semantic-nav design can't work for Rust** (and is wasteful generally): it
   spawns a fresh language server per `def`/`refs` request with a 5s timeout. rust-analyzer
   can't load a workspace in 5s, so Rust semantic nav always times out → syntactic
   fallback. Needs a persistent/warmed server, not a packaging change. **Architectural.**

## Live state observed (post-deploy)

`sulion-code --json status` for root `/home/sulion/repos/sulion`:

- `file_count=274`, `pending_file_count=274`, `symbol_count=2700`, `failed_file_count=0`
- `freshness=stale`, `latest_job = cancelled/manual` started `2026-06-13T07:26:37` (the
  pre-deploy orphan; **no new job since**)
- `semantic.available=true`; rust/typescript/tsx all `health=available`, no `last_error`
- status call latency **~44ms**
- `sulion-code refresh` **hangs 3+ minutes** (used to return instantly)
- code-intel container logs: exactly **one** line — `starting sulion code-intel service
  listen=0.0.0.0:8084 …` at `14:47:05`, then total silence (no index attempts, no errors).

## What shipped in `aa4eb59` and is VERIFIED working

1. **Status query speed** (`backend/src/code_intel/api.rs`, `load_index_summary`): was
   ~3s, now ~44ms. The old query joined `code_files` AND `code_symbols` to the root → a
   `files × symbols` Cartesian product needing `COUNT(DISTINCT)`. Rewritten to aggregate
   each table independently via `LATERAL`. **Verified via EXPLAIN ANALYZE (3s → 2ms) and
   live (44ms).**
2. **File discovery / read access** (`compose.yaml` + `code-intel/Dockerfile`): the
   container now runs as **uid/gid 7321** (the dev user that owns the ZFS datasets) instead
   of 7324. Previously it failed every cycle with `read dir /home/sulion/repos` because the
   repos root is mode `771` (NFSv4 ACL; uid 7324 = `everyone@` = traverse-but-not-list).
   As owner it reads fine. **Verified**: file_count went 82 → 274. Mount stays `:ro`.
3. **LSP binaries present** (`code-intel/Dockerfile`): added `rust-analyzer` (GitHub
   release) + `typescript-language-server` + `typescript` (npm) + Node 24. **Verified**:
   `semantic.available=true`. BUT see problem #2 — "available" ≠ "works".

## PROBLEM 1 — indexer parse hang (show-stopper, fix ready to write)

**Symptom:** 274 files discovered + marked pending, **0 parsed**, `symbols` stuck at the
old 2700, no new job, `refresh` hangs, one startup log line then silence.

**Root cause (inferred, high confidence):** the per-file index loop has no parse timeout,
so one pathological file wedges the whole indexer.

- `indexer.rs` `index_pending_files` (~L306-336) loops `for file in pending` →
  `index_pending_file` → `SourceParser::parse`.
- `index_pending_file` only logs a WARN + `mark_file_failed` on an **`Err`**. A file that
  *hangs* (never returns, never errors) produces neither — matching "silence + 0 failed".
- `parser.rs` `SourceParser::parse` (~L188-212): `self.parser.parse(source, None)` at
  **L196** — tree-sitter parse with **no timeout**. tree-sitter can spin pathologically.
- Guards that already exist and are NOT the cause: 2MB size cap + binary skip
  (`parser.rs` L8, L271), walk skips `.git/target/node_modules/dist` (L281-286). Largest
  indexable file is a normal 188KB YAML; nothing minified/huge. So it's a parse pathology,
  not size.
- **Ruled out: the LSP.** The index/refresh/startup path never calls it (see problem #2
  for where it IS called). Verified by grepping every `.lsp` reference.

**Fix:** give the parser a bounded budget so no single file can wedge the indexer. tree-
sitter is **0.25.10** (`Cargo.lock`), where `set_timeout_micros` is deprecated/gone — use
`Parser::parse_with_options` with a `progress_callback` that returns `true` (cancel) past a
wall-clock deadline. On cancel `parse()` returns `None`, which flows into the **existing**
`else` branch at `parser.rs:197` (`bail!`) → `index_pending_file` returns `Err` → file
marked failed + logged + loop continues. Self-diagnosing: after deploy the other 273 index
and the culprit shows up as a `code-intel file index failed` WARN with its path.

Pick a generous deadline (e.g. a few seconds) so normal files are unaffected. Add a test
with a pathological input. Verify locally if possible by running the parser over the repo
files; otherwise it can only be confirmed post-deploy.

## PROBLEM 2 — LSP semantic nav is architecturally unusable for Rust

**The LSP is wired into ONLY the `def`/`refs` navigation path**, never indexing. Call
sites (every other reference is just type imports):
- `backend/src/code_intel/api.rs:149` — `state.lsp.status()` (reports availability only).
- `backend/src/code_intel/api/nav.rs:267,307` — the `def`/`refs` semantic escalation.
- `code_intel.rs:74,88` construct `LspManager::default()` (struct only; no eager spawn).

**The design (the real problem):** there is **no persistent server**. Each `def`/`refs`:
- `lsp.rs` `request_locations` (~L72-124) wraps the call in `tokio::time::timeout(5s)`
  (`DEFAULT_TIMEOUT`, L14).
- `run_semantic_request` (~L300-333) → `LspClient::spawn` (L351-362) starts a **brand-new**
  rust-analyzer/tsls process (`current_dir(root)`, `kill_on_drop(true)`), sends
  `initialize` + `didOpen` + the query, then `shutdown`. Process is killed after.

So every semantic command spawns a fresh server, points it at the repo root → rust-analyzer
begins loading the **whole cargo workspace** (`cargo metadata`, proc-macro/build-script
compilation, crate-graph indexing) → waits 5s → gets killed. Rust workspace load is tens of
seconds to minutes, so **Rust `def`/`refs` will essentially always time out → syntactic
fallback**, while still paying the spawn + partial-load cost on every call.

Additional gap: the code-intel image has **no Rust toolchain** (no cargo/rustc). rust-
analyzer needs `cargo metadata` for real project semantics; without it, it degrades to
single-file mode. But adding cargo would NOT fix this — the spawn-per-request + 5s timeout
is the blocker. TypeScript (`tsserver`, needs only Node + `typescript`, both installed) may
fit under 5s for small files but has the same per-request-spawn waste.

**Fix direction:** a persistent, warmed LSP server per `(root, language)` — spawned once,
`initialize`d, kept alive and reused across requests (first request slow, rest fast), with
the 5s timeout applied only to the per-request round-trip, not the warmup. This is a real
change in `lsp.rs` (`LspManager` would own running clients, not spawn-per-call). Decide
whether Rust semantic nav is worth it (it also needs cargo/rustc in the image, making it
much heavier) or whether to keep Rust on the syntactic index and offer semantic only for TS.

## Files touched this session (relevant to code-intel)

- **Committed + deployed** (`aa4eb59`): `code-intel/Dockerfile` (uid 7321 + LSP binaries +
  Node 24), `compose.yaml` (uid via image), `backend/src/code_intel/api.rs` (status query).
- **Uncommitted / not done:** the parse-timeout fix (problem 1) and the persistent-LSP
  redesign (problem 2).

## Suggested order for the next agent

1. **Parse timeout** (problem 1) — unblocks the whole index; small, self-diagnosing.
   Deploy, confirm 273/274 index and the WARN names the bad file.
2. Decide on **LSP** (problem 2): either redesign to persistent warmed servers (+ cargo in
   the image for Rust), or scope semantic nav to TS only and stop advertising Rust as
   semantic. Until then `semantic.available=true` is misleading for Rust.

## Honesty notes

- Problem-1 root cause is **inferred** (parse hang) — strongly supported by the code paths
  and the silent-hang signature, but not reproduced. The culprit file is unidentified;
  the fix self-identifies it on deploy.
- I could not run live DB / deeper diagnostics at write time (the DB credential grant for
  this PTY had lapsed; broker 403).
- "semantic.available=true" was delivered by me adding the binaries; I initially treated
  that green status as success without checking that the per-request-spawn design makes it
  non-functional for Rust. That check should have happened before calling it done.
