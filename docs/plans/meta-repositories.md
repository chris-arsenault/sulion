# Meta-repositories and collection sessions

## Outcome

Sulion gains one metadata-only parent level above repositories. A
**meta-repository** names an ordered collection of existing repositories and one
primary repository. It supports two related product behaviors:

1. The sidebar, rail, and command palette organize repositories under
   meta-repository headers. Repositories with no parent remain under an
   `Ungrouped` header.
2. A user can start a shell, Claude, or Codex session against the whole
   collection. The session starts in the primary repository and can read and
   write every other member repository.

No checkout moves. Canonical repositories remain direct children of
`SULION_REPOS_ROOT`, and isolated worktrees remain under
`SULION_WORKSPACES_ROOT`.

## Product contract

- Meta-repositories have exactly one level. They cannot contain other
  meta-repositories.
- A repository belongs to zero or one meta-repository. Organization is a
  partition, not a tag system, so the sidebar never duplicates a repository.
- A non-empty meta-repository has one explicit primary repository. Adding the
  first member makes it primary; removing the primary promotes the first
  remaining member by display order. Empty meta-repositories may exist but
  cannot launch sessions.
- Membership and primary-repository changes affect new sessions only. Every
  session stores the exact repository/workspace set it launched with.
- One workspace mode applies to the collection. `main` binds every member's
  canonical checkout; `isolated` creates or resumes one worktree per member.
  Mixing main and isolated members in one session is not supported.
- The primary repository supplies the initial cwd and optional relative
  `working_dir`. Secondary repositories are additional roots, not subdirectories
  of a synthetic checkout.
- Collection sessions appear once, in a `Sessions` subsection on the
  meta-repository. Single-repository sessions remain under their repository.
- Repository Files, Git, Plans, and Workspaces subsections remain
  repository-specific. This change does not invent aggregate Git or file
  operations across a collection.
- Published plans remain anchored to the primary repository for now. Extending
  plans and portfolio metrics with a first-class meta-repository scope is a
  separate product decision.

## Why the primary repository is explicit

Both installed agents support additional directories:

- Claude accepts `--add-dir <directories...>`.
- Codex accepts repeatable `--add-dir <path>` alongside its primary workspace.

That lets Sulion launch from one real workspace and grant the other workspace
paths without a symlink farm. The primary still matters: it controls the shell
cwd, default Git behavior, and Codex's automatic project-root instruction and
configuration discovery. The create-session form therefore shows the primary
repository and member count before launch.

## Durable data model

Add one migration after `0067_pty_devenv_ident.sql`.

### Organization metadata

```sql
CREATE TABLE meta_repos (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    primary_repo_name TEXT,
    position INTEGER NOT NULL DEFAULT 0,
    revision BIGINT NOT NULL DEFAULT 1,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX meta_repos_name_active_uidx
    ON meta_repos (LOWER(name))
    WHERE deleted_at IS NULL;

CREATE TABLE meta_repo_members (
    meta_repo_id UUID NOT NULL REFERENCES meta_repos(id),
    repo_name TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (meta_repo_id, repo_name),
    UNIQUE (repo_name),
    UNIQUE (meta_repo_id, position)
);
```

`repo_name` deliberately does not reference `repo_runtime_state`: that table is
derived from node filesystem discovery and a temporarily missing checkout must
not erase the user's organization. The service enforces that
`primary_repo_name` names a current member and changes the group and membership
rows in one transaction.

Deleting a meta-repository soft-deletes its metadata and releases its current
members. It never deletes a checkout, workspace, plan, transcript, or PTY.

### Session scope snapshot

Keep `pty_sessions.repo` and `pty_sessions.workspace_id` as the primary
repository/workspace compatibility fields. Add the collection identity and an
immutable member snapshot:

```sql
ALTER TABLE pty_sessions
    ADD COLUMN meta_repo_id UUID REFERENCES meta_repos(id),
    ADD COLUMN meta_repo_name TEXT;

CREATE TABLE pty_session_repos (
    pty_session_id UUID NOT NULL REFERENCES pty_sessions(id),
    repo_name TEXT NOT NULL,
    workspace_id UUID REFERENCES workspaces(id),
    role TEXT NOT NULL CHECK (role IN ('primary', 'additional')),
    position INTEGER NOT NULL,
    PRIMARY KEY (pty_session_id, repo_name),
    UNIQUE (pty_session_id, position)
);

CREATE UNIQUE INDEX pty_session_repos_primary_uidx
    ON pty_session_repos (pty_session_id)
    WHERE role = 'primary';
```

Backfill one `primary` row for every existing PTY. `workspace_id` stays nullable
for historical sessions created before workspace binding existed. New sessions
always record a workspace for every member.

The snapshot prevents a group edit from changing the filesystem authority of a
running session or the scope used when an agent conversation resumes. The
snapshot name preserves a useful label if the meta-repository is later deleted.

During a control/node version-skew window, a session created by an old node may
not have a snapshot row. Readers fall back to `pty_sessions.repo` and
`workspace_id`, treating it as a single-repository session.

## API and control-plane service

Add a `meta_repos` service module and thin route handlers:

- `POST /api/meta-repos` creates a group with a name, ordered repository names,
  and primary repository.
- `PATCH /api/meta-repos/:id` renames or reorders a group with an expected
  revision.
- `PUT /api/meta-repos/:id/members` atomically replaces ordered membership and
  sets the primary repository with an expected revision.
- `DELETE /api/meta-repos/:id` soft-deletes the group and ungroups its members.

The mutation service validates names, duplicate membership, primary membership,
and current repository existence. Missing repositories remain visible in a
previously saved group but block new collection sessions until restored or
removed.

`GET /api/app-state` adds `meta_repos` rather than adding another poll. Each
entry carries id, name, revision, primary repository, ordered members, and each
member's current `exists` state. Session views add a nullable discriminated
scope while retaining the existing `repo` and `workspace` fields for rolling
browser compatibility.

Extend `POST /api/sessions` with `meta_repo_id`. A request supplies exactly one
of `repo` or `meta_repo_id`. For collection launches, the control plane loads
the current group, confirms that every member exists, and sends the node names
and preallocated workspace ids. It never resolves node-local paths.

Add `scope_source_session_id` for resume/clone flows. It reuses the source
session's stored member/workspace snapshot rather than the meta-repository's
current membership. The existing `workspace_id` flow remains the
single-repository path.

Repository rename updates `meta_repo_members`, `meta_repos.primary_repo_name`,
and `pty_session_repos` in the same lifecycle transaction that already rewrites
session/workspace names. Repository delete rejects a live session that includes
the repository in any role, removes current membership, and deterministically
promotes the next group member when needed. Historical session snapshots remain
as history.

## Node and PTY runtime

### Additive capability negotiation

Do not bump `NODE_PROTOCOL_VERSION`. The control plane deploys before the node,
so a strict bump would disconnect the still-running node. Add an absent-tolerant
`capabilities` list to authenticated heartbeats and advertise
`multi_repo_session_v1` from the new node. The new control plane continues to
serve single-repository launches through an old node and returns a specific
`503` for collection launch until the connected node reports the capability.

### Workspace allocation

Extend the node's session-create payload with optional collection metadata and
ordered member requests. The existing fields still describe the primary member,
so an old request remains valid.

For a new collection session the node:

1. Resolves and validates every member under its own `repos_root`.
2. Acquires the repository lifecycle read lock for the whole operation.
3. Ensures every main workspace or creates one isolated worktree per member.
4. Records the ordered `pty_session_repos` snapshot.
5. Starts the PTY in the primary workspace and records that workspace in the
   existing `pty_sessions.workspace_id` column.
6. If any worktree creation, database write, or PTY spawn fails, removes only
   the isolated worktrees created by this attempt and reports the member that
   failed. Existing main workspaces and pre-existing isolated workspaces are
   never rolled back.

Collection resume loads every recorded workspace and refuses the operation if
one is missing, deleted, belongs to a different node, or no longer matches its
recorded repository. It does not silently fall back to current group members or
main checkouts.

Workspace deletion and repository lifecycle checks must inspect
`pty_session_repos`, not only `pty_sessions.workspace_id` or
`pty_sessions.repo`, so a secondary workspace cannot be removed under a live
collection session.

### Agent access

Keep the primary environment variables unchanged:

- `SULION_REPO_NAME`
- `SULION_WORKSPACE_ID`
- `SULION_WORKSPACE_PATH`
- `SULION_CANONICAL_REPO`

Add collection metadata for helpers and diagnostics:

- `SULION_META_REPO_ID`
- `SULION_META_REPO_NAME`
- `SULION_REPO_NAMES_JSON`
- `SULION_REPO_PATHS_JSON`

The central `agent-launcher` reads the path list and prefixes one `--add-dir`
argument per secondary workspace for Claude and Codex. Putting this in the
launcher, rather than only in the initial create-session command, also covers a
shell-only PTY whose user later runs `cl` or `co`, plus agent resume commands.
Unit tests assert the exact argv for both agents and paths containing spaces.

The devenv already mounts `/home/sulion` at the same path as the node, so every
member path is available without a new mount or deployment change.

## Cross-repository projections and helpers

A collection session must not be multi-repository only at shell-launch time.
Two derived surfaces need the stored scope:

- File-touch projection loads all session roots and assigns an absolute touched
  path to the longest matching workspace/repository prefix. Relative paths stay
  relative to the primary cwd. The resulting `timeline_file_touches.repo_name`
  remains a real repository name, so existing file trace routes need no new
  aggregate representation.
- `sulion-retrieve` sends the PTY's repository set. Default retrieval searches
  those repository names with `repo_name = ANY(...)`; `scope=all` keeps its
  existing global meaning. Single-repository sessions retain the current scalar
  behavior.

Code intelligence remains cwd-scoped. An agent uses `cd` into a secondary
workspace before `sulion-code`; adding repo-selector flags would break the
current code-intelligence contract.

## Sidebar and navigation

Render two levels without changing the repository body:

```text
Ahara Platform
  Sessions (collection-wide)
  ahara
    Plans / Sessions / Workspaces / Files / Git
  ahara-infra
    Plans / Sessions / Workspaces / Files / Git

Ungrouped
  sulion
  tastebase
```

Interaction details:

- A sidebar-header create menu offers `New repository` and
  `New meta-repository`.
- The create/edit meta-repository dialog has a name, an ordered member
  checklist, and one primary-repository radio choice. It is the accessible
  management path; drag-and-drop is optional polish, not the only way to move a
  repository.
- A meta-repository header expands/collapses independently, shows member and
  collection-session counts, and has context actions for New session, Edit,
  Rename, and Delete.
- The collection New session form reuses agent and workspace-mode controls,
  displays the primary repository and member count, and applies the chosen mode
  to every member.
- Repository headers retain their current context menus and body. A repository
  context action can move it to another meta-repository or Ungrouped.
- Persist expansion by stable meta-repository id, not display name. Keep the
  existing repository expansion map for the inner level.
- The rail shows one sigil per meta-repository and one per ungrouped repository.
  A meta-repository sigil aggregates unread state across its collection and
  member sessions, dirty count across members, and the worst member staleness.
- Command-palette search adds meta-repository navigation and collection-session
  creation entries. Existing repository and session entries remain searchable.
- Mobile uses the same tree inside the existing drawer; no separate mobile
  information architecture is introduced.

`SessionStore` owns meta-repository data and both expansion maps because the
sidebar, rail, command palette, and session creation all consume them. Dialog
drafts remain component-local. No new React context or event-bus state is
needed.

## Phases

### Phase 1: domain, migration, service, and app-state

- Add schema, backfill, models, CRUD service, REST routes, API types, and client
  calls.
- Extend app-state with group/member data and optional session scope.
- Integrate repo rename/delete rules and version-skew fallbacks.

Exit gate: backend unit and integration tests cover CRUD, one-parent
membership, primary promotion, stale revisions, missing repos, rename/delete,
backfill, and app-state serialization. Existing single-repository API fixtures
remain valid.

### Phase 2: collection session runtime

- Add heartbeat capability advertisement and control gating.
- Extend session-create payloads and workspace allocation/rollback.
- Persist exact session scopes and resume the stored workspace set.
- Add PTY environment values and central Claude/Codex `--add-dir` injection.
- Update live-session refusal checks for every member workspace/repository.

Exit gate: node protocol integration tests prove old-node single-repo
compatibility, explicit old-node collection refusal, main and isolated
collection launch, partial-failure cleanup, and exact-scope resume.

### Phase 3: navigation and management UI

- Add store state/actions, meta-repository dialog, two-level sidebar rendering,
  collection session rows/forms, rail aggregation, and palette entries.
- Preserve repository body behavior and stable callback/subscription patterns.

Exit gate: component tests cover grouped/ungrouped rendering, expansion
persistence, membership edits, primary choice, collection create requests,
session placement, error rendering, rail aggregation, and mobile drawer use.

### Phase 4: file-touch and retrieval scope

- Resolve touches against every stored session root.
- Extend retrieval context and lexical/semantic filters to a repository set.
- Keep code-intelligence cwd inference unchanged and document the `cd` behavior.

Exit gate: ingestion fixtures attribute writes in primary and secondary repos
correctly; retrieval integration tests return member-repo history by default and
exclude unrelated repositories.

### Phase 5: end-to-end verification and documentation

- Add a real-stack Playwright path that creates a meta-repository from seeded
  repos, launches a mock agent collection session, and verifies navigation and
  persisted hierarchy after reload.
- Extend the launcher-backed fixture to capture the effective additional
  directories without invoking live Claude or Codex.
- Update `docs/architecture.md`, `docs/state-management.md`,
  `docs/design.md`, `docs/development.md`, `docs/ingestion.md`,
  `docs/node-protocol.md`, `docs/retrieval.md`, `docs/user-guide.md`, and
  `docs/e2e-coverage-plan.md` to describe the shipped contract.
- Run `make ci`, `make test-rust-integration`, `make e2e`, and
  `make validate-deploy`.

Exit gate: all four commands pass. Deployment remains unverified until a
separately authorized push and post-deploy smoke test.

## Failure behavior

- Empty meta-repository: disable New session and explain that it needs a member.
- Missing member: show the repository as missing and reject launch before any
  worktree is created.
- Disconnected node: keep organization/history readable; mutation returns the
  existing development-node unavailable response.
- Old connected node: single-repository launch works; collection launch says
  that the node must finish updating.
- Partial isolated creation: name the failing member and clean up only the new
  worktrees from this attempt.
- Stale edit dialog: reject the old revision and reload current membership
  instead of overwriting a newer change.
- Deleted group with historical sessions: retain the snapshot label and exact
  repository set on the session; do not recreate the group.

## Explicitly out of scope

- Moving repositories into nested filesystem directories.
- Nested meta-repositories or overlapping repository membership.
- Cloning or creating several repositories as one operation.
- Aggregate stage, commit, diff, file tree, or branch operations.
- Cross-repository atomic Git commits.
- Loading every secondary repository's project-specific agent configuration as
  if it were the primary project. Agent-native `--add-dir` grants filesystem
  access; the selected primary remains the project root.
- Meta-repository-scoped plans, metrics, secrets, or device-ingest routes.
- Live Claude/Codex execution in automated tests.
