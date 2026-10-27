# ADR 0011: Session Workspaces Are the Canonical Checkout or a Sulion-Created Git Worktree

## Status

Accepted, 2026-05-03.

## Context

Until May 2026 every PTY session for a repository started in the same
directory, the canonical checkout under the repos root. Two agents working
the same repo at once edited the same files and shared one `git status`,
so running several agents in parallel meant serialising them by hand.

The user asked for a way to "bind a session to a shadow copy of the repo
on a different branch (or the main copy if I decide)" that is isolated and
transparent to the agents, with the agents able to learn where they are so
that a session on `main` can handle branching, merging, and fast-forwards.
The request named a backend filesystem shadow as one possible shape.

## Decision

A **workspace** is the filesystem checkout bound to a PTY session. It is
one of two kinds:

- `main` — the canonical repo checkout.
- `worktree` — a Git worktree Sulion creates with `git worktree add` under
  the workspaces root, on its own branch.

Each `pty_sessions` row carries a `workspace_id`; the PTY's cwd is the
workspace path. Session creation offers `workspace_mode = main | isolated`.
The PTY receives `SULION_WORKSPACE_*` metadata (kind, path, canonical repo,
branch, base ref) so an agent that asks can tell where it is. Agents see an
ordinary checkout; there is no wrapper around filesystem access.

Repo-scoped file and git routes target the canonical checkout.
Workspace-scoped routes under `/api/workspaces/:id/*` target the
session-bound checkout, and file and diff tabs opened from a session follow
its workspace. Sulion surfaces dirty and ahead/behind state and lets the
user delete a worktree; it does not merge on the user's behalf.

Collection sessions over a meta-repository always run with
`workspace_mode = main`. The primary repo's canonical checkout is the cwd
and the other members are passed to the agent as additional directories;
Sulion creates no worktrees, symlinks, or temporary directories for them.
Only the primary workspace is bound to the session.

## Alternatives considered

- **A filesystem overlay or copy layer.** The user's request mentioned a
  "shadow copy" and a "backend file system shadow". The proposal that
  became 95119b0 (transcript 019db1c2…, turn 30136207) rejected inventing
  an overlay or copy layer: a Git worktree already gives each agent a real
  directory, a real branch, a real `git status`, isolated uncommitted
  files, and cheap creation while sharing the object store. Full clones
  would have paid for a second object store per session for nothing the
  worktree does not provide.
- **Branch switching inside the canonical checkout.** The same proposal
  keeps `main` as a distinct kind bound directly to the canonical path
  rather than switching its branch per session, so the integrator session
  and any explicit main-worktree session share one checkout and isolated
  work never moves the canonical HEAD.
- **Automatic merging by Sulion.** Rejected in the proposal ("Sulion
  should not auto-merge. It should surface state and provide safe
  visibility."). Branching and merging stay explicit, done by a person or
  by an agent running in the main workspace.
- **One isolated worktree per member for collection sessions.** Built in
  the first meta-repository branch merged as 3340d12, together with
  per-session member snapshots and rollback deletion. The user had already
  called that design over-engineered (transcript 019fe439…, turn 5748935),
  and fb5fc58 removed it the same day: collection sessions use canonical
  roots only, and `docs/meta-repositories.md` records the reduced
  contract.
- **A `worktree` alias for `workspace_mode`.** An undocumented spelling of
  `isolated` that the API accepted. Dropped in ebc3731 because the error
  text and the frontend type only ever named `main` and `isolated`; the
  persisted workspace kind `worktree` is a different field and stays.

## Consequences

- Several agents can work one repository concurrently, each on its own
  branch, and an integrator on `main` can review and merge their work with
  ordinary Git.
- Worktrees accumulate. Deletion is a user action, and cleanup of orphaned
  worktree directories and rows has needed operator intervention when the
  cleanup path lagged a deploy.
- Every path that assumed `repos_root/<repo>` had to be audited; the split
  between repo-scoped and workspace-scoped routes is the lasting shape of
  that audit, and ADR 0002 later carried both over the node protocol.
- The worktrees root is a separate dataset (and on the dedicated node a
  separate directory), which deployment and backup have to know about.

## References

- [`../architecture.md`](../architecture.md) — Session model, Workspace
- [`../user-guide.md`](../user-guide.md) — Create and resume sessions
- [`../meta-repositories.md`](../meta-repositories.md)
- 95119b0 — workspace isolation and runner workflows
- 3340d12 — first meta-repositories merge
- fb5fc58 — simplify meta-repository sessions to canonical roots
- ebc3731 — drop the `worktree` workspace-mode alias
