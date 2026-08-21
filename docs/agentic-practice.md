# What Sulion does

Agentic development converged on a set of practices during 2026: keep the
transcript rather than summarizing it, retrieve instead of stuffing context,
prefer structural code lookups over vector similarity, broker credentials
instead of exporting them, and make a long-running agent's state legible to the
person supervising it.

This page maps those practices onto what Sulion implements. Component pointers
into `backend/src/` and `frontend/src/` are the source of truth; the shape of
the system is in [`architecture.md`](architecture.md).

## Keeps the whole transcript

The agent's JSONL transcript is the authoritative record and Postgres is the
query layer over it. The ingester is the only component that reads JSONL; REST
and WebSocket paths query Postgres.

Claude and Codex translate into one canonical event schema
(`backend/src/ingest/canonical/`), which then projects into app-shaped timeline
tables (`backend/src/ingest/timeline/`, `backend/src/ingest/projection.rs`).
Two agent families, one vocabulary for turns, operations, references, and file
touches.

Idempotency keys on `(session_uuid, byte_offset)`. Append-only transcripts make
byte offset stable, so the ingester resumes from its last committed offset after
any interruption and re-reads are free. `claude_sessions.parent_session_uuid`
carries compaction lineage, so a compacted child links back to the session it
descends from.

The timeline is the review surface: filterable by turn, tool category, error
state, and file path, virtualized for sessions running to thousands of events,
and exportable as markdown.

## Retrieves from history instead of re-explaining it

`sulion-retrieve` searches past sessions with hybrid lexical and semantic
matching, scoped to the current repo by default. Agents reach it as a PTY
command; the CLI supplies auth and context headers from the environment. Full
contract in [`retrieval.md`](retrieval.md).

Search results are curated rather than exhaustive. `include` defaults to
assistant text. Tool traffic enters results only when asked for by category or
name, and shell, edit-payload, and inspection mechanics stay behind
`include_low_value=true`.

Semantic indexing follows the same rule. Only natural-language reasoning and
short intent get embedded: assistant, user, and summary text, subagent finals,
tool names with capped input, and capped error messages. Command output, file
reads, diffs, image payloads, and MCP state dumps are excluded — the code index
already covers that ground.

Freshness is tracked per source key in `retrieval_embedding_sources` with
durable cursor backfills, so the index converges without a request-path scan.
`sulion-retrieve index-status` reports pending, indexed, failed, and deleted
counts.

## Answers code questions structurally

`sulion-code` gives agents symbol-level navigation instead of grep-and-hope:
`outline`, `find`, `def`, `refs`, `search`, `patch`, and `pack`. Contract in
[`code-intel.md`](code-intel.md); the durable decision is
[ADR-0001](adrs/0001-code-intelligence-agent-tool.md).

Semantic resolution is the primary path for Rust, TypeScript, TSX, and
JavaScript, backed by persistent language servers for recently active roots.
Any syntactic fallback is labeled as such in the response.

Index state is reported, not hidden. Navigation routes read the current index
and say when a result is stale or partial rather than indexing inside the
request. `sulion-code refresh` marks a root dirty and records deletions; the
background worker drains pending files idempotently.

Output size is a first-class parameter: `--budget small|normal|large` lets an
agent ask for the amount of context the task warrants.

## Keeps secrets out of the agent's environment

The broker is a separate service with its own database, storing encrypted
bundles and grant state apart from application data. The backend does not hold
the master key. Full trust boundary in [`secrets.md`](secrets.md).

`with-cred -- <command>` injects secrets as environment variables for the
lifetime of one spawned command. Nothing lands in shell startup files, `.env`
files, or logs. Grants are scoped to a `(PTY, secret)` pair with a TTL and are
managed from terminal and session context menus.

`with-cred` and the `aws` wrapper are the supported consumption paths. A refusal
is legible: exit code 66, with stderr naming the `(pty, tool, secret_id)` tuple
that was denied, so the operator can grant it deliberately.

The shipped agent instructions treat that refusal as a boundary. An agent that
hits a credential failure reports it and stops, rather than searching SSM, env
files, shell history, or other repos for a substitute token.

## Bounds what containers can do

In brokered mode the runner is the only Sulion container holding the host Docker
socket, and it is a command broker rather than an API proxy. PTYs see a `docker`
wrapper that forwards cwd, PTY id, and argv; the runner applies policy before
executing: supported subcommands only, Sulion labels on created containers,
resource defaults, no privileged mode, no host namespaces, no added
capabilities, no devices, no bind mounts, automatic attachment to the `sulion`
network, and no interactive sessions. Compose commands route through a shim that
maps the default network onto the same external network.

On the dedicated host, `SULION_DOCKER_MODE=direct` execs the real CLI against
the system daemon. The node passes the socket and its numeric host GID only to
the devenv containers it launches, so non-root PTY processes inherit access
without a hard-coded group. The control plane never receives that socket.

## Keeps sessions alive across everything

A PTY session is a long-lived shell on the server that survives client
disconnect and ends on explicit delete, shell exit, or reboot.

The devenv server owns the PTY masters and shadow emulators, dialing the node
over a unix socket on the shared run volume. On the dedicated host it is a
label-owned container the node launches and adopts, so recreating `sulion-node`
leaves every shell running and the devenv redials the new node.

The shadow `vt100` emulator is fed continuously whether or not a client is
attached, which is what makes snapshot-on-connect land in a populated buffer.
Attaching from a phone, laptop, or desktop mirrors the same live shell.

Isolated workspaces bind a session to a Sulion-created Git worktree on its own
branch, so parallel sessions in one repo work on separate checkouts.

## Makes agent state legible without inference

Two separate signals, both reported rather than guessed at. Full contract in
[`plans.md`](plans.md).

**Published plans** are durable, repo-scoped phase summaries an agent writes
with `sulion plan`. Phases carry a title, description, status, optional note,
and t-shirt size. A plan attaches to multiple live PTYs, survives terminal exit,
and stays in the repo's closed-plan history. It is the user-facing projection
alongside whatever internal planning the agent does for itself.

**Terminal activity** answers what a PTY is doing now. `shell` and `starting`
derive from the process; `working`, `awaiting_prompt`, `needs_input`, `blocked`,
and `unknown` are stored states. Claude hooks and Codex lifecycle events report
working and awaiting transitions automatically. An agent narrows that with
`sulion activity working|waiting|blocked`, and an explicit `needs_input` or
`blocked` holds until the next explicit transition rather than being cleared by
an automatic turn-complete signal.

Agents may also name their own terminal with `sulion name`, shown beside the
user's label.

## Presents the whole fleet on one screen

The Overview tab projects every live PTY from `/api/app-state`, including shells
with no browser tab open. Repos read as teams and sessions as engineer cards,
with teams needing attention sorted first.

Each card carries operational activity, attached plan and current phase, latest
timeline output, agent uptime, cumulative token spend, average token rate, and
context remaining once the agent has reported enough to compute it. Missing
telemetry reads as unavailable rather than estimated.

The Metrics tab aggregates the portfolio: non-overlapping input, cached-input,
and output rollups; a reconciled 14-day series from `agent_model_usage_daily`;
per-model usage priced at standard-tier list rates with unknown models left
visibly unpriced; git activity per repo with agent and human commit attribution
from `Co-Authored-By` trailers; churn hotspots from repeated file writes; and
plan-flow charts replayed from `plan_events`.

## Ships the instructions that make the tooling visible

An agent uses `with-cred`, `sulion-retrieve`, `sulion-code`, and `sulion plan`
when it knows they exist. [`agent-instructions/`](agent-instructions/README.md)
carries user-level `CLAUDE.md` and `AGENTS.md` templates that teach them, along
with the working practices proven out on Sulion-hosted sessions: edit
discipline, credential boundaries, deployment-failure handling, planning
defaults.

The two templates carry the same policies, phrased per agent where the tooling
differs.

## Further reading

External material behind the practices above:

- [Effective context engineering for AI agents — Anthropic](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
- [Context engineering: a practical guide — Sourcegraph](https://sourcegraph.com/blog/context-engineering)
- [AgentGUI: observing and steering long-running agents (arXiv 2607.26300)](https://arxiv.org/html/2607.26300)
- [Agentic harness engineering (arXiv 2604.25850)](https://arxiv.org/pdf/2604.25850)
- [The conversations beneath the code (arXiv 2605.02244)](https://arxiv.org/html/2605.02244)
- [How to sandbox AI agents — Northflank](https://northflank.com/blog/how-to-sandbox-ai-agents)
