# User-wide agent instructions

Templates for the user-level instruction files that make the sulion PTY tooling
visible to coding agents. The `with-cred` broker, `sulion-retrieve`,
`sulion-code`, and `sulion plan` are only used when the agent knows they exist —
these files are how it learns. They also carry a set of general working
practices (edit discipline, credential boundaries, deployment-failure handling,
working climate) proven out on sulion-hosted sessions.

| File | Install to | Agent |
| ---- | ---- | ---- |
| [`CLAUDE.md`](CLAUDE.md) | `~/.claude/CLAUDE.md` | Claude Code |
| [`AGENTS.md`](AGENTS.md) | `~/.codex/AGENTS.md` | Codex |

## Installing

These tracked templates are the maintained source. Copy the appropriate file
to its install path after editing it, then require a byte-identical `diff`
between the template and live file. The templates carry the same policies;
only product-specific tool mappings differ: Claude names Artifact and
Edit/Write, while Codex names its native patch/editor operation.

These files guide agent behavior; they are not a client-side enforcement
boundary. Restrictions that require a guarantee also belong in tool
permissions, hooks, or the surrounding runtime.

The templates are written in the first person — "I" and "me" refer to you, the
adopting user, since these files speak to the agent in your voice. Adjust any
section that doesn't match how you work; the `with-cred` and sulion CLI
sections are the environment-load-bearing parts.

## Section guide

- **File edits through agent tooling** — keeps every write visible to sulion's
  file-churn tracking; shell-based writes bypass it.
- **No hosted presentation publishing** — keeps user data local unless the
  current request explicitly authorizes external publication.
- **Secrets via `with-cred`** — brokered, single-command secret injection; see
  [docs/secrets.md](../secrets.md) for the trust boundary.
- **Credential boundaries** — auth failures are a stop-and-report boundary,
  never a prompt to hunt for substitute credentials.
- **`sulion-retrieve` / `sulion-code` / `sulion plan`** — transcript search,
  structural code navigation, and published phase summaries.
- **Git, planning, deployment-failure handling, working climate** — general
  practices; keep or adapt to taste.
