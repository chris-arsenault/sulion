# ADR 0018: Meta-Repositories as Metadata-Only Groups

## Status

Accepted, 2026-08-09.

## Context

The dedicated node holds many repositories, and a platform such as Ahara is
nine of them. The user asked for a single-level organization of repositories
in the sidebar and the ability to open a Claude or Codex session against a
collection of repositories, stating up front that the file structure need not
move: "this can all be done in metadata."

The first implementation (`1e9a6cd`, `96699b7`) read "collection" as a
collection of managed workspaces. It stored a normalized member snapshot per
session and backfilled every historical session, allocated one Git worktree
per member for isolated collection sessions with rollback on failure, added
group and member ordering, soft deletion, revision checks, and a heartbeat
capability map to preflight collection launches, and widened file-touch
projection and retrieval scope to repository sets. A reduction review found
4,692 added lines across 61 files for two user-visible capabilities. The user
confirmed the direction was wrong: the grouping had to happen at the logical
level, not the filesystem level, to avoid that churn.

## Decision

A meta-repository is a metadata-only group above repositories. It changes
navigation and new-session scope and never moves checkouts or creates a
filesystem hierarchy.

- Groups are one level deep with a unique name. A repository belongs to
  zero or one group. Every group has one explicit primary repository.
- Create and update replace name, membership, and primary in one
  transaction; delete is a hard delete with `ON DELETE SET NULL` on
  historical sessions. Groups and members sort alphabetically.
- A collection session always uses `workspace_mode=main`. Its cwd and
  workspace metadata come from the primary's canonical checkout. Secondary
  members are canonical repo roots passed to Claude and Codex as additional
  directories (`--add-dir`). Sulion creates no worktrees, symlinks, leases,
  or temporary collection directories.
- A session stores only its primary `repo`, primary `workspace_id`, and a
  nullable `meta_repo_id`. Group edits affect later launches and resumes; a
  running shell keeps the environment it started with.
- File projection and transcript retrieval keep their primary-repository
  scope.

The control plane resolves names and membership; the node resolves them under
its own `repos_root`. The session-create message gains optional group
metadata and secondary names through Serde defaults, with no heartbeat
feature negotiation (ADR 0016). Overview groups member repositories'
terminals and plans as a presentation grouping of existing app-state data.

## Alternatives considered

- **Per-member worktrees and durable scope snapshots per collection
  session.** The pre-simplification shape, removed by `fb5fc58`. The review
  identified it as the largest source of excess: a resumable workspace-set
  abstraction with `scope_source_session_id`, historical backfill, and
  multi-worktree rollback, built to preserve exact historical membership and
  isolated-mode parity nobody had asked for. The user's correction named the
  filesystem-level approach as the churn to avoid.
- **Moving checkouts into a group directory.** Excluded by the request
  itself: the initial planning did not require migrating the file structure
  into a hierarchy, and `docs/meta-repositories.md` keeps that boundary. SMB
  clients and existing worktrees depend on the flat `repos_root` layout.
- **Nested or overlapping groups.** The request was for a single-level
  organization; nesting is listed out of scope in the design doc, and
  one-parent membership is enforced by a unique `repo_name` constraint.
- **Ordered members, soft deletion, and optimistic concurrency on group
  CRUD.** Rejected by the review: no UI ordered groups and no restore path
  existed, and the frontend's two-request save could commit a rename while
  a membership change failed. One transactional `PUT` replaced it.
- **Heartbeat capability negotiation to preflight collection launch.**
  Rejected; the protocol's existing "unsupported" response for an unknown
  request kind already covers an older node, and the additive request
  fields need no negotiation.
- **Widening retrieval and file-touch projection to the repository set.**
  Rejected as an optional expansion that changed nine files and left
  relative paths still anchored to the primary; both keep their existing
  primary-repo scope until cross-repository history is requested.

## Consequences

- A collection session is an ordinary primary-repo session with extra
  `--add-dir` arguments and four exported `SULION_*` variables; every
  existing per-repo subsystem keeps working unchanged.
- Deleting a group leaves its sessions as ordinary primary-repo history.
- Isolated-workspace collection sessions are not offered; the collection
  form omits workspace mode.
- Group membership changes are not visible inside a running shell until the
  session is relaunched or resumed.
- Cross-repository commits, aggregate Git or file-tree views, and
  collection-wide retrieval remain out of scope.

## References

- Current contract: [`../meta-repositories.md`](../meta-repositories.md);
  session model in [`../architecture.md`](../architecture.md#session-model)
- Protocol rule the launch fields follow:
  [ADR 0016](0016-additive-node-protocol-without-version-negotiation.md)
- Commits: `1e9a6cd` (domain and API), `96699b7` (collection launch),
  `fb5fc58` (simplification to metadata-only), `d84c621` (Overview
  grouping), `ce5d08e` (documentation and verification)
- Transcript: session `019fe43b-133b-7be1-bdee-9398a136538b`, turns `89673`
  (request), `5556644` (reduction review), `6253379` (user correction)
- CHANGELOG Unreleased, "Workspace and sessions"
