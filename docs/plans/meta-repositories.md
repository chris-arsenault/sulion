# Meta-repositories and collection sessions

## Outcome

A meta-repository is one logical grouping above repositories. It organizes the
sidebar, rail, and command palette and can launch a session with access to every
current member. It does not move checkouts or create a filesystem hierarchy.

## Contract

- Groups are one level deep and have a unique name.
- A repository belongs to zero or one group.
- Every group has at least one member and one explicit primary repository.
- Groups and members sort alphabetically; users do not manage positions.
- Create and update replace the name, membership, and primary repository in one
  transaction. Delete removes the group immediately.
- A collection session always uses `workspace_mode=main`. Its cwd and ordinary
  workspace metadata come from the primary canonical checkout.
- Secondary members are canonical repo roots passed to Claude and Codex with
  `--add-dir`. Sulion creates no worktrees, symlinks, leases, or temporary
  collection directories for them.
- A session stores only its primary `repo`, primary `workspace_id`, and nullable
  `meta_repo_id`. There is no per-session member or workspace snapshot.
- Group edits affect later launches and resumes. A running shell keeps the
  environment it received when it started.
- If a group is deleted, its foreign key is cleared from historical sessions;
  those sessions remain ordinary primary-repo history.
- File projection and transcript retrieval retain their existing primary-repo
  scope. Collection access does not widen those subsystems.

## Data model

Migration `0068_meta_repositories.sql` adds:

- `meta_repos(id, name, primary_repo_name, created_at, updated_at)`;
- `meta_repo_members(meta_repo_id, repo_name)` with unique `repo_name`; and
- nullable `pty_sessions.meta_repo_id` with `ON DELETE SET NULL`.

The service checks that every member currently exists, the primary is a member,
and no member belongs to another group. Repository rename updates membership and
the primary name in the existing lifecycle transaction. Repository deletion
removes membership, promotes the alphabetically first remaining member, or
deletes the now-empty group.

## API

- `POST /api/meta-repos` creates a complete group.
- `PUT /api/meta-repos/:id` replaces a complete group.
- `DELETE /api/meta-repos/:id` hard-deletes a group.
- `GET /api/app-state` includes alphabetically sorted `meta_repos` and the
  nullable group identity on each session.
- `POST /api/sessions` accepts exactly one of `repo` or `meta_repo_id`.

The control plane resolves names and current membership. The node resolves those
names under its own `repos_root`; no node-local path crosses the control
protocol.

## Runtime

The existing session-create message keeps its scalar primary-repo fields and
adds optional group metadata plus a list of secondary repo names. The node:

1. rejects collection modes other than `main`;
2. resolves the primary repo and normal main workspace;
3. resolves each distinct secondary name to its canonical root;
4. starts the PTY in the primary workspace; and
5. binds only the primary workspace to the session.

The PTY exports `SULION_META_REPO_ID`, `SULION_META_REPO_NAME`,
`SULION_REPO_NAMES_JSON`, and `SULION_REPO_PATHS_JSON`. The central agent
launcher prefixes one `--add-dir` per secondary path, including when an agent is
started later from a shell-only PTY.

No heartbeat feature negotiation is needed. The additive request fields use
Serde defaults, so ordinary single-repo requests retain their existing shape.

## Navigation

Collection sessions render once under the group. Member repos keep their own
plans, repo sessions, workspaces, files, and Git actions. Ungrouped repos remain
top-level. The rail aggregates group status and the command palette can reveal a
group or open its collection-session form. Mobile uses the same tree in the
existing drawer.

The group editor selects members and one primary; it has no ordering controls.
The collection session form omits workspace mode because canonical roots are the
only supported mode.

## Verification

- Backend integration coverage: atomic CRUD, one-parent membership, rename and
  delete behavior, primary promotion, canonical-root launch, and isolated-mode
  rejection.
- Frontend component coverage: grouped navigation, atomic editing, primary
  selection, and main-only collection requests.
- Playwright coverage: create a group, launch a collection shell, verify the
  repo-root environment, run the deterministic agent fixture, and reload.

## Out of scope

- Nested or overlapping groups.
- Per-member workspaces or collection worktree cleanup.
- Stored launch scopes or snapshot-based resume.
- Aggregate Git, file-tree, plan, metric, or retrieval operations.
- Cross-repository atomic commits.
