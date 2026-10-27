# Architecture Decision Records

One file per decision that chose between real alternatives. Each record holds
the context at the time, the decision, the alternatives rejected with their
evidence, and the consequences. Records are not edited after acceptance; a
later decision that changes one is a new record whose status names the one it
amends or supersedes. Numbers are allocated in order of writing, so the
sequence is not chronological: 0001–0003 were written as the decisions were
made, and 0004–0030 were backfilled from commit history, plan records, and
session transcripts on 2026-10-27.

Add a record when a change picks between approaches that a later reader
could reasonably have expected to see chosen differently. A change with no
real alternative gets a sentence in the topic doc instead.

| # | Title | Status | Date |
| - | ----- | ------ | ---- |
| [0001](0001-code-intelligence-agent-tool.md) | Agent-facing code intelligence | Accepted | 2026-06-01 |
| [0002](0002-hybrid-control-plane-and-dev-node.md) | Hybrid control plane and development node | Accepted; amended by 0015, 0017 | 2026-07-27 |
| [0003](0003-tiered-transcript-retention.md) | Tiered transcript retention | Accepted | 2026-09-22 |
| [0004](0004-live-terminal-and-projected-history.md) | Live terminal pane and transcript-projected history are separate pipelines | Accepted | 2026-04-18 |
| [0005](0005-web-workspace-over-server-shell-and-shared-postgres.md) | Browser workspace over a server-side general shell on shared Postgres | Accepted | 2026-04-18 |
| [0006](0006-canonical-event-schema.md) | Canonical event schema with per-agent parsers | Accepted | 2026-04-19 |
| [0007](0007-zustand-stores-and-typed-command-layer.md) | Zustand stores, a typed app command layer, and a thin tab registry | Accepted | 2026-04-19 |
| [0008](0008-real-stack-e2e-with-mock-agents.md) | End-to-end tests run against the real stack with mock agent launchers | Accepted | 2026-04-20 |
| [0009](0009-separate-secret-broker-with-signed-redemption.md) | A separate secret broker with per-PTY signed redemption | Accepted | 2026-04-24 |
| [0010](0010-cognito-authentication-with-broker-held-revocation.md) | Cognito authentication, fail-closed outages, and a broker-held revocation cutoff | Accepted | 2026-10-14 |
| [0011](0011-workspace-isolation-with-git-worktrees.md) | Session workspaces are the canonical checkout or a Sulion-created Git worktree | Accepted | 2026-05-03 |
| [0012](0012-retrieval-embeds-natural-language-only.md) | Semantic retrieval embeds chunked natural-language content only | Accepted | 2026-06-22 |
| [0013](0013-usage-accounting-and-pricing-catalog.md) | Usage counted once per message with a priced model catalog carrying provenance | Accepted | 2026-10-09 |
| [0014](0014-published-plans-and-branch-plans.md) | Published plans are a compact projection; branches nest at the plan level | Accepted | 2026-10-22 |
| [0015](0015-userspace-tls-node-endpoint-instead-of-wireguard.md) | Userspace TLS node endpoint instead of WireGuard | Accepted; amends 0002 | 2026-07-28 |
| [0016](0016-additive-node-protocol-without-version-negotiation.md) | Additive node protocol without version negotiation | Accepted | 2026-07-29 |
| [0017](0017-devenv-server-owns-pty-masters.md) | Devenv server owns PTY masters | Accepted; amends 0002 | 2026-07-30 |
| [0018](0018-meta-repositories-as-metadata-only-groups.md) | Meta-repositories as metadata-only groups | Accepted | 2026-08-09 |
| [0019](0019-display-modes-and-tabs-rendered-by-reference.md) | Display modes and tabs rendered by reference | Accepted | 2026-08-21 |
| [0020](0020-library-and-future-prompts-in-postgres.md) | Library and future prompts in Postgres | Accepted | 2026-08-21 |
| [0021](0021-claude-code-seeded-natively-into-persistent-home.md) | Claude Code seeded natively into the persistent home | Accepted | 2026-09-11 |
| [0022](0022-model-switch-guard.md) | Unconfirmed model switches interrupt the turn and ask in the timeline | Accepted | 2026-09-17 |
| [0023](0023-turn-digest-is-prompt-text-and-tool-headers.md) | The stored turn digest holds the prompt, assistant text, and one header per tool call | Accepted | 2026-09-20 |
| [0024](0024-incremental-per-event-timeline-projection.md) | The timeline is projected incrementally from per-event items with a single reducer | Accepted | 2026-09-24 |
| [0025](0025-progressive-turn-loading-over-ndjson.md) | Turn detail streams as finite NDJSON with keyset pagination and a shared bounded cache | Accepted | 2026-09-25 |
| [0026](0026-foreground-s3-upload-staging.md) | Remote uploads stage through a private presigned S3 bucket as foreground operations | Accepted | 2026-09-25 |
| [0027](0027-remove-device-access-and-allowlist-the-public-edge.md) | Device-token access is deleted, machine routes are LAN-only, and the public edge forwards an allowlist | Accepted | 2026-09-28 |
| [0028](0028-with-cred-as-the-only-credential-path.md) | with-cred is the only credential path; grants are scoped to a terminal, a repository, or every terminal bound to programs | Accepted | 2026-10-01 |
| [0029](0029-dora-metrics-from-deploy-pipeline-evidence.md) | DORA metrics are aggregated from the known deploy pipeline's evidence, portfolio-wide only | Accepted | 2026-10-15 |
| [0030](0030-standalone-mobile-install-without-service-worker.md) | Home-screen installation without a service worker or offline cache | Accepted | 2026-10-16 |
