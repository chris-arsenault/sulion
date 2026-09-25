# Implementation plan index

Reviewed against repository history through `c3fe6a5` on 2026-09-21.
These files record implementation work and proposals. Current product contracts
belong in the durable docs linked below; published progress is managed with
[`sulion plan`](../plans.md).

## In progress

- [Remote file uploads through S3](remote-file-uploads.md): foreground
  browser-to-S3 transfer and node installation, retaining WAF restrictions.
  The simplified implementation is locally validated; publication remains pending.

## Pending proposal

- [Database archive, backup, and monthly purge](transcript-archive-and-purge.md)
  remains a proposal with M0–M5 pending. Encryption, retention windows,
  maintenance authorization, and process placement remain user decisions.
  September 20 projection changes require new sizing before implementation.

## Completed implementation records

- [Progressive turn loading](progressive-turn-loading.md): bounded streaming,
  shared incremental caching, local filters and on-demand tool bodies. Local
  verification and fixture measurements are recorded; browser and deployed LAN
  latency/buffering checks remain unverified.

- [PTY survival](archive/pty-survives-deploy.md), with
  [phase 1](archive/pty-survives-deploy-phase1.md),
  [phase 2](archive/pty-survives-deploy-phase2.md), and
  [phase 3](archive/pty-survives-deploy-phase3.md), shipped in `8104eb8`,
  `bcc6e82`, and `6cd80c3`. The records retain original steps and historical
  verification. Use [architecture](../architecture.md#pty-lifetime-and-toolset-upgrades)
  and [deployment](../deploy.md#pty-and-toolset-releases) for current behavior.
- [Cleanup and hardening](archive/cleanup-and-hardening.md) records the July
  cleanup series. Its remaining compatibility retirements and host-repair
  retirement gate are retained in [the backlog](../backlog.md#deferred-maintenance).
  Completion of the published plan did not satisfy those operational gates.

The completed meta-repository plan already described the final contract. It now
lives in [Meta-repositories and collection sessions](../meta-repositories.md),
including the August simplification and Overview grouping.

Archived instructions and line numbers describe their implementation period.
Do not execute them as a current work queue or treat their test counts as a
fresh validation result.
