// Sidebar: repo-centric navigation (Sessions / Files / Git) scrolls in
// the middle; the cross-repo Library (References / Prompts) is pinned
// to the bottom so it stays reachable regardless of how far the user
// has scrolled the repo list. StatsStrip sits beneath Library.

import {
  type FormEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useShallow } from "zustand/react/shallow";

import type {
  AgentLaunchType,
  DirEntryView,
  GitCommit,
  RepoGitSummary,
  RepoView,
  SecretGrantMetadata,
  SessionColor,
  SessionView,
  WorkspaceView,
} from "../api/types";
import { SESSION_COLORS } from "../api/types";
import { ApiError, stageRepoPath, uploadRepoFile } from "../api/client";
import { appCommands, useAppCommand } from "../state/AppCommands";
import { useSessions } from "../state/SessionStore";
import { dirtyAncestors, stalenessFor, useRepos } from "../state/RepoStore";
import { useSecretStore } from "../state/SecretStore";
import type { TabStore } from "../state/TabStore";
import { useTabs } from "../state/TabStore";
import { Icon } from "../icons";
import { Tooltip } from "./ui";
import type { MenuItem } from "./common/contextMenuStore";
import {
  contextMenuHandler,
  contextMenuTriggerProps,
  useContextMenu,
} from "./common/contextMenuStore";
import { buildSecretContextMenu } from "./common/secretContextMenu";
import { ConfirmDialog } from "./common/ConfirmDialog";
import { LibraryPanel } from "./LibraryPanel";
import { ReindexButton } from "./ReindexButton";
import { StatsStrip } from "./StatsStrip";
import "./Sidebar.css";
import "./LibrarySection.css";

const EMPTY_SECRET_GRANTS: SecretGrantMetadata[] = [];

type NewSessionFormValue = {
  working_dir: string;
  launch_agent: AgentLaunchType | "";
  workspace_mode: "isolated" | "main";
};

export function Sidebar() {
  const {
    sessions,
    repos,
    selectedSessionId,
    workspaces,
    selectSession,
    createSession,
    deleteSession,
    deleteWorkspace,
    updateSession,
    createRepo,
    isUnread,
    repoExpansion,
    setRepoExpanded,
    collapseRepos,
  } = useSessions(
    useShallow((store) => ({
      sessions: store.sessions,
      repos: store.repos,
      selectedSessionId: store.selectedSessionId,
      workspaces: store.workspaces,
      selectSession: store.selectSession,
      createSession: store.createSession,
      deleteSession: store.deleteSession,
      deleteWorkspace: store.deleteWorkspace,
      updateSession: store.updateSession,
      createRepo: store.createRepo,
      isUnread: store.isUnread,
      repoExpansion: store.repoExpansion,
      setRepoExpanded: store.setRepoExpanded,
      collapseRepos: store.collapseRepos,
    })),
  );

  const openTab = useTabs((store) => store.openTab);

  const grouped = useMemo(
    () => groupByRepo(sessions, repos, workspaces),
    [sessions, repos, workspaces],
  );

  // Opening a session's work area: terminal top + timeline bottom.
  // Called directly from the click handler (no useEffect on selected
  // session) so sidebar interaction doesn't fight file/tab
  // activation through the global tab state.
  const openSessionTabs = useCallback(
    (id: string) => {
      selectSession(id);
      openTab({ kind: "terminal", sessionId: id }, "top");
      openTab({ kind: "timeline", sessionId: id }, "bottom");
      appCommands.closeDrawer();
    },
    [openTab, selectSession],
  );
  const expandedByRepo = useMemo(
    () =>
      Object.fromEntries(
        grouped.map((group) => [
          group.name,
          repoExpansion[group.name] ?? defaultRepoExpanded(group),
        ]),
      ),
    [grouped, repoExpansion],
  );
  const collapseTargetRepos = useMemo(() => {
    const emptyExpanded = grouped
      .filter((group) => group.sessions.length === 0 && expandedByRepo[group.name])
      .map((group) => group.name);
    if (emptyExpanded.length > 0) return emptyExpanded;
    return grouped
      .filter((group) => expandedByRepo[group.name])
      .map((group) => group.name);
  }, [expandedByRepo, grouped]);
  const collapseButtonLabel = useMemo(() => {
    const hasExpandedEmptyRepo = grouped.some(
      (group) => group.sessions.length === 0 && expandedByRepo[group.name],
    );
    return hasExpandedEmptyRepo
      ? "Collapse repos without sessions"
      : "Collapse all repos";
  }, [expandedByRepo, grouped]);
  const [newRepoOpen, setNewRepoOpen] = useState(false);
  const [newSessionFor, setNewSessionFor] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [pendingWorkspaceDelete, setPendingWorkspaceDelete] = useState<{
    workspace: WorkspaceView;
    force: boolean;
  } | null>(null);
  const [revealRequest, setRevealRequest] = useState<{
    repo: string;
    path: string;
    nonce: number;
  } | null>(null);

  useAppCommand("reveal-file", ({ repo, path }) => {
    setRevealRequest({ repo, path, nonce: Date.now() });
  });

  const repoAnchorsRef = useRef<Map<string, HTMLLIElement>>(new Map());
  const registerRepoAnchor = useCallback(
    (name: string, el: HTMLLIElement | null) => {
      if (el) repoAnchorsRef.current.set(name, el);
      else repoAnchorsRef.current.delete(name);
    },
    [],
  );

  useAppCommand("reveal-repo", ({ repo }) => {
    const el = repoAnchorsRef.current.get(repo);
    if (el) {
      requestAnimationFrame(() =>
        el.scrollIntoView({ block: "start", behavior: "smooth" }),
      );
    }
  });

  const toggleRepo = useCallback(
    (name: string, currentlyExpanded: boolean) =>
      setRepoExpanded(name, !currentlyExpanded),
    [setRepoExpanded],
  );
  const collapseRepoGroups = useCallback(
    () => collapseRepos(collapseTargetRepos),
    [collapseRepos, collapseTargetRepos],
  );

  const onCreateRepo = useCallback(
    async (form: { name: string; git_url: string }) => {
      setFormError(null);
      try {
        await createRepo({
          name: form.name,
          git_url: form.git_url.trim() || undefined,
        });
        setNewRepoOpen(false);
      } catch (err) {
        setFormError(messageOf(err));
      }
    },
    [createRepo],
  );

  const onCreateSession = useCallback(
    async (
      repoName: string,
      form: NewSessionFormValue,
    ) => {
      setFormError(null);
      try {
        await createSession({
          repo: repoName,
          working_dir: form.working_dir.trim() || undefined,
          workspace_mode: form.workspace_mode,
          launch_agent: form.launch_agent || undefined,
        });
        setNewSessionFor(null);
      } catch (err) {
        setFormError(messageOf(err));
      }
    },
    [createSession],
  );

  const requestDelete = useCallback(
    (id: string) => setPendingDeleteId(id),
    [],
  );
  const confirmDelete = useCallback(async () => {
    const id = pendingDeleteId;
    if (!id) return;
    setPendingDeleteId(null);
    try {
      await deleteSession(id);
    } catch (err) {
      setFormError(messageOf(err));
    }
  }, [deleteSession, pendingDeleteId]);

  const resumeWorkspace = useCallback(
    async (workspace: WorkspaceView) => {
      setFormError(null);
      try {
        const created = await createSession({
          repo: workspace.repo_name,
          workspace_id: workspace.id,
        });
        selectSession(created.id);
        openTab({ kind: "terminal", sessionId: created.id }, "top");
        openTab({ kind: "timeline", sessionId: created.id }, "bottom");
        appCommands.closeDrawer();
      } catch (err) {
        setFormError(messageOf(err));
      }
    },
    [createSession, openTab, selectSession],
  );

  const requestWorkspaceDelete = useCallback(
    (workspace: WorkspaceView, force = false) =>
      setPendingWorkspaceDelete({ workspace, force }),
    [],
  );

  const confirmWorkspaceDelete = useCallback(async () => {
    const pending = pendingWorkspaceDelete;
    if (!pending) return;
    setPendingWorkspaceDelete(null);
    try {
      await deleteWorkspace(pending.workspace.id, {
        force: pending.force,
        deleteBranch: true,
      });
    } catch (err) {
      setFormError(messageOf(err));
    }
  }, [deleteWorkspace, pendingWorkspaceDelete]);

  const onUpdateSession = useCallback(
    async (id: string, patch: Parameters<typeof updateSession>[1]) => {
      setFormError(null);
      try {
        await updateSession(id, patch);
      } catch (err) {
        setFormError(messageOf(err));
      }
    },
    [updateSession],
  );

  const cancelPendingDelete = useCallback(
    () => setPendingDeleteId(null),
    [],
  );
  const cancelPendingWorkspaceDelete = useCallback(
    () => setPendingWorkspaceDelete(null),
    [],
  );

  const toggleNewRepoOpen = useCallback(() => {
    setNewRepoOpen((v) => !v);
    setNewSessionFor(null);
  }, []);
  const closeNewRepo = useCallback(() => setNewRepoOpen(false), []);

  const setNewSessionForFn = useCallback(
    (next: (prev: string | null) => string | null) => setNewSessionFor(next),
    [],
  );
  const repoGroupHandlers = useMemo<RepoGroupBaseHandlers>(
    () => ({
      toggleRepo,
      setNewSessionFor: setNewSessionForFn,
      createSession: onCreateSession,
      registerAnchor: registerRepoAnchor,
    }),
    [toggleRepo, setNewSessionForFn, onCreateSession, registerRepoAnchor],
  );
  const repoGroupSelection = useMemo<RepoGroupSelection>(
    () => ({
      selectedSessionId,
      onSelectSession: openSessionTabs,
    }),
    [selectedSessionId, openSessionTabs],
  );
  const repoGroupSessionOps = useMemo<RepoGroupSessionOps>(
    () => ({
      onRequestDelete: requestDelete,
      onUpdateSession,
    }),
    [requestDelete, onUpdateSession],
  );
  const repoGroupWorkspaceOps = useMemo<RepoGroupWorkspaceOps>(
    () => ({
      onResumeWorkspace: resumeWorkspace,
      onRequestDeleteWorkspace: requestWorkspaceDelete,
    }),
    [requestWorkspaceDelete, resumeWorkspace],
  );

  return (
    <div className="sidebar">
      <div className="sidebar__header">
        <span className="sidebar__logo">sulion</span>
        <div className="sidebar__header-actions">
          <Tooltip label={collapseButtonLabel}>
            <button
              type="button"
              className="sidebar__icon-button"
              onClick={collapseRepoGroups}
              aria-label={collapseButtonLabel}
              disabled={collapseTargetRepos.length === 0}
            >
              <Icon name="panel-left-close" size={14} />
            </button>
          </Tooltip>
          <Tooltip label="New repo">
            <button
              type="button"
              className="sidebar__icon-button"
              onClick={toggleNewRepoOpen}
              aria-label="New repo"
            >
              <Icon name="plus" size={14} />
            </button>
          </Tooltip>
        </div>
      </div>

      {newRepoOpen && (
        <NewRepoForm onSubmit={onCreateRepo} onCancel={closeNewRepo} />
      )}

      {formError && <div className="sidebar__error">{formError}</div>}

      <div className="sidebar__scroll">
        {grouped.length === 0 && (
          <div className="sidebar__muted">No repos yet.</div>
        )}

        <ul className="sidebar__tree">
          {grouped.map((group) => (
            <RepoGroup
              key={group.name}
              group={group}
              expanded={expandedByRepo[group.name] ?? defaultRepoExpanded(group)}
              newSessionRepoName={newSessionFor}
              handlers={repoGroupHandlers}
              selection={repoGroupSelection}
              sessionOps={repoGroupSessionOps}
              workspaceOps={repoGroupWorkspaceOps}
              isUnread={isUnread}
              onError={setFormError}
              revealRequest={
                revealRequest?.repo === group.name ? revealRequest : null
              }
            />
          ))}
        </ul>
      </div>
      {pendingDeleteId && (
        <ConfirmDialog
          title="Delete session?"
          message="This terminates the shell and marks the session deleted. Any running command loses its process."
          confirmLabel="Delete"
          destructive
          onConfirm={confirmDelete}
          onCancel={cancelPendingDelete}
        />
      )}
      {pendingWorkspaceDelete && (
        <ConfirmDialog
          title={
            pendingWorkspaceDelete.force
              ? "Force delete workspace?"
              : "Delete workspace?"
          }
          message={
            pendingWorkspaceDelete.force
              ? "This removes the Git worktree, deletes the Sulion branch, and discards uncommitted workspace changes."
              : "This removes the Git worktree and deletes the Sulion branch if it has no unmerged work."
          }
          confirmLabel={pendingWorkspaceDelete.force ? "Force delete" : "Delete"}
          destructive
          onConfirm={confirmWorkspaceDelete}
          onCancel={cancelPendingWorkspaceDelete}
        />
      )}
      <LibraryPanel />
      <StatsStrip />
      <div className="sidebar__admin">
        <ReindexButton />
      </div>
    </div>
  );
}

interface RepoGroupData {
  name: string;
  exists: boolean;
  git: RepoGitSummary | null;
  timelineRevision: number;
  sessions: SessionView[];
  workspaces: WorkspaceView[];
}

function groupByRepo(
  sessions: SessionView[],
  repos: RepoView[],
  workspaces: WorkspaceView[],
): RepoGroupData[] {
  const byName = new Map<string, RepoGroupData>();
  for (const r of repos) {
    byName.set(r.name, {
      name: r.name,
      exists: r.exists ?? true,
      git: r.git ?? null,
      timelineRevision: r.timeline_revision ?? 0,
      sessions: [],
      workspaces: [],
    });
  }
  for (const s of sessions) {
    if (!byName.has(s.repo)) {
      byName.set(s.repo, {
        name: s.repo,
        exists: false,
        git: null,
        timelineRevision: 0,
        sessions: [],
        workspaces: [],
      });
    }
    byName.get(s.repo)!.sessions.push(s);
  }
  for (const workspace of workspaces) {
    if (workspace.kind === "main" || workspace.state === "deleted") continue;
    if (!byName.has(workspace.repo_name)) {
      byName.set(workspace.repo_name, {
        name: workspace.repo_name,
        exists: false,
        git: null,
        timelineRevision: 0,
        sessions: [],
        workspaces: [],
      });
    }
    byName.get(workspace.repo_name)!.workspaces.push(workspace);
  }
  for (const g of byName.values()) {
    g.sessions.sort(sessionCompare);
    g.workspaces.sort(workspaceCompare);
  }
  return Array.from(byName.values()).sort((a, b) => a.name.localeCompare(b.name));
}

function defaultRepoExpanded(group: RepoGroupData): boolean {
  return group.sessions.length > 0 || group.workspaces.length > 0;
}

function sessionCompare(a: SessionView, b: SessionView): number {
  const ap = a.pinned ? 1 : 0;
  const bp = b.pinned ? 1 : 0;
  if (ap !== bp) return bp - ap;
  return new Date(b.created_at).getTime() - new Date(a.created_at).getTime();
}

function workspaceCompare(a: WorkspaceView, b: WorkspaceView): number {
  const dirtyA = a.git.uncommitted_count > 0 ? 1 : 0;
  const dirtyB = b.git.uncommitted_count > 0 ? 1 : 0;
  if (dirtyA !== dirtyB) return dirtyB - dirtyA;
  return new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime();
}

// ─── Repo group ─────────────────────────────────────────────────────

interface RepoGroupSelection {
  selectedSessionId: string | null;
  onSelectSession: (id: string) => void;
}

interface RepoGroupSessionOps {
  onRequestDelete: (id: string) => void;
  onUpdateSession: (
    id: string,
    patch: {
      label?: string | null;
      pinned?: boolean;
      color?: SessionColor | null;
    },
  ) => void | Promise<void>;
}

interface RepoGroupWorkspaceOps {
  onResumeWorkspace: (workspace: WorkspaceView) => void | Promise<void>;
  onRequestDeleteWorkspace: (
    workspace: WorkspaceView,
    force?: boolean,
  ) => void;
}

/** Base handlers — stable at the parent level and curried per-repo by
 * RepoGroup itself. Keeping the curry inside the group avoids creating
 * new closure/object literals in the parent map body on every render. */
interface RepoGroupBaseHandlers {
  toggleRepo: (name: string, currentlyExpanded: boolean) => void;
  setNewSessionFor: (next: (prev: string | null) => string | null) => void;
  createSession: (
    repoName: string,
    form: NewSessionFormValue,
  ) => void | Promise<void>;
  registerAnchor: (name: string, el: HTMLLIElement | null) => void;
}

interface RepoGroupProps {
  group: RepoGroupData;
  expanded: boolean;
  newSessionRepoName: string | null;
  handlers: RepoGroupBaseHandlers;
  selection: RepoGroupSelection;
  sessionOps: RepoGroupSessionOps;
  workspaceOps: RepoGroupWorkspaceOps;
  isUnread: (sessionId: string, lastEventAt: string | null) => boolean;
  onError: (message: string | null) => void;
  revealRequest: { repo: string; path: string; nonce: number } | null;
}

function RepoGroup({
  group,
  expanded,
  newSessionRepoName,
  handlers,
  selection,
  sessionOps,
  workspaceOps,
  isUnread,
  onError,
  revealRequest,
}: RepoGroupProps) {
  const openTab = useTabs((store) => store.openTab);
  const openCtx = useContextMenu((store) => store.open);
  const { selectedSessionId, onSelectSession } = selection;
  const { onRequestDelete, onUpdateSession } = sessionOps;
  const { onResumeWorkspace, onRequestDeleteWorkspace } = workspaceOps;
  const { toggleRepo, setNewSessionFor, createSession, registerAnchor } =
    handlers;
  const anchorRef = useCallback(
    (el: HTMLLIElement | null) => registerAnchor(group.name, el),
    [registerAnchor, group.name],
  );

  const onToggle = useCallback(
    () => toggleRepo(group.name, expanded),
    [toggleRepo, group.name, expanded],
  );
  const newSessionOpen = newSessionRepoName === group.name;
  const newSessionOnStart = useCallback(
    () =>
      setNewSessionFor((prev) => (prev === group.name ? null : group.name)),
    [setNewSessionFor, group.name],
  );
  const newSessionOnSubmit = useCallback(
    (form: NewSessionFormValue) =>
      createSession(group.name, form),
    [createSession, group.name],
  );
  const newSessionOnCancel = useCallback(
    () => setNewSessionFor(() => null),
    [setNewSessionFor],
  );
  const newSessionOnStartClick = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      newSessionOnStart();
    },
    [newSessionOnStart],
  );

  const toggleSessionsSub = useCallback(
    () => setSubOpen((p) => ({ ...p, sessions: !p.sessions })),
    [],
  );
  const toggleFilesSub = useCallback(
    () => setSubOpen((p) => ({ ...p, files: !p.files })),
    [],
  );
  const toggleGitSub = useCallback(
    () => setSubOpen((p) => ({ ...p, gitSection: !p.gitSection })),
    [],
  );
  const git = group.git;
  const [subOpen, setSubOpen] = useState({
    sessions: true,
    workspaces: true,
    files: false,
    gitSection: true,
  });
  const toggleWorkspacesSub = useCallback(
    () => setSubOpen((p) => ({ ...p, workspaces: !p.workspaces })),
    [],
  );

  // Reveal requests (agent file-touches, "Reveal in file tree" menu,
  // etc.) should respect the Files subsection's current toggle: if
  // the user has it collapsed, stay collapsed. The tree itself still
  // auto-expands ancestor directories when Files is open — that's
  // gated by the subOpen.files render check below.

  // Compute staleness colour: needs the max event age across sessions
  // in this repo versus the last-commit time.
  const latestEventAt = useMemo(() => {
    let max = 0;
    for (const s of group.sessions) {
      if (s.last_event_at) {
        const t = new Date(s.last_event_at).getTime();
        if (t > max) max = t;
      }
    }
    return max > 0 ? max : null;
  }, [group.sessions]);
  const staleness = stalenessFor(git, latestEventAt);
  const onRepoContextMenu = useMemo(
    () =>
      contextMenuHandler(openCtx, () => [
        {
          kind: "item" as const,
          id: "open-repo-timeline",
          label: "Open repo timeline",
          onSelect: () => {
            openTab({ kind: "timeline", repo: group.name }, "bottom");
            appCommands.closeDrawer();
          },
        },
        {
          kind: "item" as const,
          id: "open-repo-diff",
          label: "Open repo diff",
          onSelect: () => {
            openTab({ kind: "diff", repo: group.name });
            appCommands.closeDrawer();
          },
        },
      ]),
    [openCtx, openTab, group.name],
  );

  return (
    <li className="sidebar__group" ref={anchorRef} data-repo-name={group.name}>
      <div className="sidebar__group-header">
        <button
          type="button"
          className="sidebar__group-toggle"
          onClick={onToggle}
          onContextMenu={onRepoContextMenu}
        >
          <span
            className={
              expanded
                ? "sidebar__chevron sidebar__chevron--open"
                : "sidebar__chevron"
            }
          >
            <Icon name="chevron-right" size={12} />
          </span>
          <span className="sidebar__group-name">{group.name}</span>
          {git && <RepoBadge git={git} staleness={staleness} />}
        </button>
      </div>

      {expanded && (
        <div className="sidebar__repo-body">
          <Subsection
            label="Sessions"
            open={subOpen.sessions}
            onToggle={toggleSessionsSub}
            count={group.sessions.length}
            rightSlot={
              <Tooltip label={`New session in ${group.name}`}>
                <button
                  type="button"
                  className="sidebar__icon-button"
                  aria-label={`New session in ${group.name}`}
                  onClick={newSessionOnStartClick}
                  disabled={!group.exists}
                >
                  <Icon name="plus" size={14} />
                </button>
              </Tooltip>
            }
          >
            {newSessionOpen && (
              <NewSessionForm
                repoName={group.name}
                onSubmit={newSessionOnSubmit}
                onCancel={newSessionOnCancel}
              />
            )}
            {group.sessions.length === 0 && (
              <div className="sidebar__muted">— no sessions —</div>
            )}
            {group.sessions.map((s) => (
              <SessionRow
                key={s.id}
                session={s}
                selected={s.id === selectedSessionId}
                unread={isUnread(s.id, s.last_event_at)}
                onSelect={onSelectSession}
                onDelete={onRequestDelete}
                onUpdate={onUpdateSession}
              />
            ))}
          </Subsection>

          <Subsection
            label="Workspaces"
            open={subOpen.workspaces}
            onToggle={toggleWorkspacesSub}
            count={group.workspaces.length}
          >
            {group.workspaces.length === 0 && (
              <div className="sidebar__muted">— no isolated workspaces —</div>
            )}
            {group.workspaces.map((workspace) => (
              <WorkspaceRow
                key={workspace.id}
                workspace={workspace}
                sessions={group.sessions}
                onResume={onResumeWorkspace}
                onRequestDelete={onRequestDeleteWorkspace}
              />
            ))}
          </Subsection>

          <Subsection label="Files" open={subOpen.files} onToggle={toggleFilesSub}>
            {subOpen.files && (
              <FileTree
                repoName={group.name}
                gitSummary={git}
                onError={onError}
                revealRequest={revealRequest}
              />
            )}
          </Subsection>

          <Subsection
            label="Git"
            open={subOpen.gitSection}
            onToggle={toggleGitSub}
          >
            <GitPanel git={git} />
          </Subsection>
        </div>
      )}
    </li>
  );
}

function Subsection({
  label,
  open,
  onToggle,
  count,
  rightSlot,
  children,
}: {
  label: string;
  open: boolean;
  onToggle: () => void;
  count?: number;
  rightSlot?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="sidebar__sub">
      <div className="sidebar__sub-header">
        <button
          type="button"
          className="sidebar__sub-toggle"
          onClick={onToggle}
        >
          <span
            className={
              open
                ? "sidebar__chevron sidebar__chevron--open"
                : "sidebar__chevron"
            }
          >
            <Icon name="chevron-right" size={12} />
          </span>
          <span className="sidebar__sub-label">{label}</span>
          {count != null && (
            <span className="sidebar__sub-count">{count}</span>
          )}
        </button>
        {rightSlot}
      </div>
      {open && <div className="sidebar__sub-body">{children}</div>}
    </div>
  );
}

function RepoBadge({
  git,
  staleness,
}: {
  git: RepoGitSummary;
  staleness: "green" | "amber" | "red";
}) {
  const age = git.last_commit ? relativeAge(git.last_commit.committed_at) : "—";
  const tooltip = git.last_commit
    ? `${git.branch ?? "detached"} · ${git.uncommitted_count} uncommitted · last ${age}\n“${git.last_commit.subject}”`
    : `${git.branch ?? "detached"} · no commits`;
  return (
    <Tooltip label={tooltip}>
      <span
        className={`sidebar__repo-badge sidebar__repo-badge--${staleness}`}
        data-staleness={staleness}
      >
        <span className="sidebar__repo-branch">
          <Icon name="git-branch" size={12} />
          <span>{git.branch ?? "—"}</span>
        </span>
        {git.uncommitted_count > 0 && (
          <span className="sidebar__repo-dot">
            <Icon name="dirty" size={12} />
            <span className="tabular">{git.uncommitted_count}</span>
          </span>
        )}
        <span className="sidebar__repo-age tabular">{age}</span>
      </span>
    </Tooltip>
  );
}

// ─── Workspace row ─────────────────────────────────────────────────

interface WorkspaceRowProps {
  workspace: WorkspaceView;
  sessions: SessionView[];
  onResume: (workspace: WorkspaceView) => void | Promise<void>;
  onRequestDelete: (workspace: WorkspaceView, force?: boolean) => void;
}

function WorkspaceRow({
  workspace,
  sessions,
  onResume,
  onRequestDelete,
}: WorkspaceRowProps) {
  const openCtx = useContextMenu((store) => store.open);
  const openTab = useTabs((store) => store.openTab);
  const linkedSession = workspace.created_by_session_id
    ? sessions.find((session) => session.id === workspace.created_by_session_id)
    : null;
  const dirtyCount = workspace.git.uncommitted_count;
  const displayBranch = workspace.branch_name ?? "isolated";
  const displayName = displayBranch.replace(/^sulion\//, "");
  const meta = [
    workspace.state,
    dirtyCount > 0 ? `${dirtyCount} dirty` : "clean",
    linkedSession ? `session ${linkedSession.id.slice(0, 8)}` : "no session",
    ageSince(workspace.updated_at),
  ].join(" · ");

  const openDiff = useCallback(() => {
    openTab({
      kind: "diff",
      repo: workspace.repo_name,
      workspaceId: workspace.id,
    });
    appCommands.closeDrawer();
  }, [openTab, workspace.id, workspace.repo_name]);
  const resume = useCallback(() => {
    void onResume(workspace);
  }, [onResume, workspace]);
  const requestDelete = useCallback(
    () => onRequestDelete(workspace, false),
    [onRequestDelete, workspace],
  );
  const requestForceDelete = useCallback(
    () => onRequestDelete(workspace, true),
    [onRequestDelete, workspace],
  );
  const menuItems = useMemo<MenuItem[]>(
    () => [
      {
        kind: "item",
        id: "resume-workspace",
        label: "Resume workspace",
        onSelect: resume,
      },
      {
        kind: "item",
        id: "open-workspace-diff",
        label: "Open workspace diff",
        onSelect: openDiff,
      },
      { kind: "separator" },
      {
        kind: "item",
        id: "delete-workspace",
        label: "Delete workspace",
        destructive: true,
        onSelect: requestDelete,
      },
      {
        kind: "item",
        id: "force-delete-workspace",
        label: "Force delete workspace",
        destructive: true,
        onSelect: requestForceDelete,
      },
    ],
    [openDiff, requestDelete, requestForceDelete, resume],
  );
  const buildMenuItems = useCallback(() => menuItems, [menuItems]);
  const { onContextMenu, onKeyDown } = useMemo(
    () => contextMenuTriggerProps(openCtx, buildMenuItems),
    [buildMenuItems, openCtx],
  );

  return (
    <div className="sidebar__workspace-row">
      <Tooltip label="Open workspace diff">
        <button
          type="button"
          className="sidebar__workspace-main"
          onClick={openDiff}
          onContextMenu={onContextMenu}
          onKeyDown={onKeyDown}
        >
          <span className="sidebar__workspace-icon" aria-hidden>
            <Icon name="layers" size={14} />
          </span>
          <span className="sidebar__workspace-text">
            <span className="sidebar__workspace-name">{displayName}</span>
            <span className="sidebar__workspace-meta tabular">{meta}</span>
          </span>
        </button>
      </Tooltip>
      <div className="sidebar__workspace-actions">
        <Tooltip label="Resume workspace">
          <button
            type="button"
            className="sidebar__icon-button"
            aria-label={`Resume workspace ${displayName}`}
            onClick={resume}
          >
            <Icon name="terminal" size={14} />
          </button>
        </Tooltip>
        <Tooltip label="Open workspace diff">
          <button
            type="button"
            className="sidebar__icon-button"
            aria-label={`Open workspace diff ${displayName}`}
            onClick={openDiff}
          >
            <Icon name="diff" size={14} />
          </button>
        </Tooltip>
        <Tooltip label="Delete workspace">
          <button
            type="button"
            className="sidebar__icon-button sidebar__icon-button--danger"
            aria-label={`Delete workspace ${displayName}`}
            onClick={requestDelete}
          >
            <Icon name="trash-2" size={14} />
          </button>
        </Tooltip>
      </div>
    </div>
  );
}

// ─── File tree ──────────────────────────────────────────────────────

function FileTree({
  repoName,
  gitSummary,
  onError,
  revealRequest,
}: {
  repoName: string;
  gitSummary: RepoGitSummary | null;
  onError: (message: string | null) => void;
  revealRequest: { repo: string; path: string; nonce: number } | null;
}) {
  const { state, loadDir, hardRefresh, setShowAll, expandPath, loadDirty } = useRepos(
    useShallow((store) => ({
      state: store.repos[repoName],
      loadDir: store.loadDir,
      hardRefresh: store.hardRefresh,
      setShowAll: store.setShowAll,
      expandPath: store.expandPath,
      loadDirty: store.loadDirty,
    })),
  );

  useEffect(() => {
    if (state?.tree[""] === undefined) {
      loadDir(repoName, "");
    }
  }, [loadDir, repoName, state?.tree]);

  useEffect(() => {
    if (!revealRequest) return;
    expandPath(repoName, revealRequest.path);
  }, [expandPath, repoName, revealRequest]);

  useEffect(() => {
    loadDirty(repoName, gitSummary);
  }, [gitSummary, loadDirty, repoName]);

  const dirtyExpand = useMemo(
    () => dirtyAncestors(state?.git?.dirty_by_path ?? {}),
    [state?.git?.dirty_by_path],
  );

  const onShowAllChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) =>
      setShowAll(repoName, e.target.checked),
    [setShowAll, repoName],
  );

  const onHardRefresh = useCallback(() => {
    hardRefresh(repoName);
  }, [hardRefresh, repoName]);

  const root = state?.tree[""];
  if (root === undefined) {
    return <div className="sidebar__muted">loading…</div>;
  }
  if (root === null) {
    return <div className="sidebar__muted">loading…</div>;
  }

  return (
    <div className="sidebar__tree-body">
      <div className="sidebar__tree-toolbar">
        <Tooltip label={`Hard refresh ${repoName}`}>
          <button
            type="button"
            className="sidebar__icon-button"
            aria-label={`Hard refresh ${repoName}`}
            onClick={onHardRefresh}
          >
            <Icon name="refresh-cw" size={14} />
          </button>
        </Tooltip>
      </div>
      <TreeNodes
        repoName={repoName}
        path=""
        entries={root.entries}
        dirtyExpand={dirtyExpand}
        depth={0}
        onError={onError}
        revealRequest={revealRequest}
      />
      <label className="sidebar__tree-toggle">
        <input
          type="checkbox"
          checked={state?.showAll ?? false}
          onChange={onShowAllChange}
        />
        show all (incl. ignored)
      </label>
    </div>
  );
}

function TreeNodes({
  repoName,
  path,
  entries,
  dirtyExpand,
  depth,
  onError,
  revealRequest,
}: {
  repoName: string;
  path: string;
  entries: DirEntryView[];
  dirtyExpand: Set<string>;
  depth: number;
  onError: (message: string | null) => void;
  revealRequest: { repo: string; path: string; nonce: number } | null;
}) {
  return (
    <ul className="sidebar__tree-list">
      {entries.map((e) => {
        const childPath = path ? `${path}/${e.name}` : e.name;
        return (
          <TreeRow
            key={childPath}
            repoName={repoName}
            entry={e}
            fullPath={childPath}
            dirtyExpand={dirtyExpand}
            depth={depth}
            onError={onError}
            revealRequest={revealRequest}
          />
        );
      })}
    </ul>
  );
}

function TreeRow({
  repoName,
  entry,
  fullPath,
  dirtyExpand,
  depth,
  onError,
  revealRequest,
}: {
  repoName: string;
  entry: DirEntryView;
  fullPath: string;
  dirtyExpand: Set<string>;
  depth: number;
  onError: (message: string | null) => void;
  revealRequest: { repo: string; path: string; nonce: number } | null;
}) {
  const { state, loadDir, toggleDir, refresh } = useRepos(
    useShallow((store) => ({
      state: store.repos[repoName],
      loadDir: store.loadDir,
      toggleDir: store.toggleDir,
      refresh: store.refresh,
    })),
  );
  const [dragOver, setDragOver] = useState(false);
  const openCtx = useContextMenu((store) => store.open);
  const uploadInputRef = useRef<HTMLInputElement | null>(null);
  const rowRef = useRef<HTMLButtonElement | null>(null);

  const userExpanded = state?.expanded?.has(fullPath) ?? false;
  const userCollapsed = state?.collapsed?.has(fullPath) ?? false;
  const autoExpanded = dirtyExpand.has(fullPath);
  // User collapse wins over auto-expand-on-dirty. User expand wins
  // when set. Otherwise fall back to auto-expand.
  const isExpanded =
    entry.kind === "dir" &&
    (userCollapsed ? false : userExpanded || autoExpanded);

  useEffect(() => {
    if (entry.kind === "dir" && isExpanded) {
      loadDir(repoName, fullPath);
    }
  }, [entry.kind, fullPath, isExpanded, loadDir, repoName]);

  const isRevealTarget = revealRequest?.path === fullPath;
  const liveDirty =
    entry.kind === "file"
      ? (state?.git?.dirty_by_path[fullPath] ?? null)
      : entry.dirty;
  const liveDiff =
    entry.kind === "file"
      ? (state?.git?.diff_stats_by_path[fullPath] ?? entry.diff)
      : entry.diff;
  useEffect(() => {
    if (!isRevealTarget) return;
    rowRef.current?.scrollIntoView({ block: "center" });
  }, [isRevealTarget, revealRequest?.nonce]);

  const onClickRow = useCallback(() => {
    if (entry.kind === "dir") {
      toggleDir(repoName, fullPath, isExpanded);
    } else {
      appCommands.openFile({ repo: repoName, path: fullPath });
    }
  }, [entry.kind, toggleDir, repoName, fullPath, isExpanded]);

  const openFileTab = useCallback(
    () => appCommands.openFile({ repo: repoName, path: fullPath }),
    [repoName, fullPath],
  );

  const copyPath = useCallback(
    (variant: "absolute" | "relative") => {
      const text =
        variant === "absolute"
          ? `/home/dev/repos/${repoName}/${fullPath}`
          : fullPath;
      void navigator.clipboard?.writeText(text).catch(() => {
        /* ignore — HTTP deploys lack permission */
      });
    },
    [repoName, fullPath],
  );

  const onContextMenu = contextMenuHandler(openCtx, () => {
    const items: MenuItem[] = [];
    if (entry.kind === "file") {
      items.push({
        kind: "item",
        id: "open-file",
        label: "Open file",
        onSelect: openFileTab,
      });
      if (entry.dirty) {
        items.push({
          kind: "item",
          id: "open-diff",
          label: "Open diff",
          onSelect: () => appCommands.openDiff({ repo: repoName, path: fullPath }),
        });
      }
      items.push({ kind: "separator" });
    } else {
      // dir
      items.push({
        kind: "item",
        id: "toggle",
        label: isExpanded ? "Collapse" : "Expand",
        onSelect: () => toggleDir(repoName, fullPath, isExpanded),
      });
      items.push({
        kind: "item",
        id: "upload",
        label: "Upload files here…",
        onSelect: () => uploadInputRef.current?.click(),
      });
      items.push({ kind: "separator" });
    }
    items.push({
      kind: "item",
      id: "copy-rel",
      label: "Copy relative path",
      onSelect: () => copyPath("relative"),
    });
    items.push({
      kind: "item",
      id: "copy-abs",
      label: "Copy absolute path",
      onSelect: () => copyPath("absolute"),
    });
    if (entry.dirty && entry.kind === "file") {
      items.push({ kind: "separator" });
      const staged = entry.dirty[0] !== " " && entry.dirty[0] !== "?";
      items.push({
        kind: "item",
        id: "stage",
        label: staged ? "Unstage" : "Stage",
        onSelect: async () => {
          try {
            await stageRepoPath(repoName, fullPath, !staged);
            refresh(repoName);
          } catch {
            /* swallowed; ref the error through the store next refresh */
          }
        },
      });
    }
    return items;
  });

  const onUploadChange = useCallback(
    async (e: React.ChangeEvent<HTMLInputElement>) => {
      const files = Array.from(e.target.files ?? []);
      for (const f of files) {
        try {
          await uploadRepoFile(repoName, fullPath, f);
        } catch (err) {
          onError(`Upload failed for ${f.name}: ${messageOf(err)}`);
        }
      }
      refresh(repoName);
      e.target.value = "";
    },
    [onError, refresh, repoName, fullPath],
  );

  const isDir = entry.kind === "dir";
  const onDirDragOver = useCallback((ev: React.DragEvent) => {
    if (ev.dataTransfer.types.includes("Files")) {
      ev.preventDefault();
      setDragOver(true);
    }
  }, []);
  const onDirDragLeave = useCallback(() => setDragOver(false), []);
  const onDirDrop = useCallback(
    async (ev: React.DragEvent) => {
      ev.preventDefault();
      setDragOver(false);
      const files = Array.from(ev.dataTransfer.files);
      for (const f of files) {
        try {
          await uploadRepoFile(repoName, fullPath, f);
        } catch (err) {
          onError(`Upload failed for ${f.name}: ${messageOf(err)}`);
        }
      }
      refresh(repoName);
    },
    [onError, refresh, repoName, fullPath],
  );
  const dropHandlers = useMemo(
    () =>
      isDir
        ? {
            onDragOver: onDirDragOver,
            onDragLeave: onDirDragLeave,
            onDrop: onDirDrop,
          }
        : {},
    [isDir, onDirDragOver, onDirDragLeave, onDirDrop],
  );

  const childEntries = state?.tree[fullPath];

  const rowStyle = useMemo(
    () => ({ paddingLeft: 4 + depth * 12 }),
    [depth],
  );
  const tooltip = liveDirty ? `${liveDirty.trim()} ${fullPath}` : fullPath;
  return (
    <li className="sidebar__tree-item">
      <Tooltip label={tooltip}>
        <button
          ref={rowRef}
          type="button"
          className={
            "sidebar__tree-row" +
            (entry.kind === "dir" ? " sidebar__tree-row--dir" : "") +
            (dragOver ? " sidebar__tree-row--drag-over" : "") +
            (liveDirty ? " sidebar__tree-row--dirty" : "") +
            (isRevealTarget ? " sidebar__tree-row--revealed" : "")
          }
          data-repo={repoName}
          data-path={fullPath}
          data-kind={entry.kind}
          // eslint-disable-next-line local/no-inline-styles -- depth is per-row; can't be expressed as a finite class set
          style={rowStyle}
          onClick={onClickRow}
          onContextMenu={onContextMenu}
          {...dropHandlers}
        >
          {entry.kind === "dir" && (
            <span
              className={
                isExpanded
                  ? "sidebar__chevron sidebar__chevron--open"
                  : "sidebar__chevron"
              }
            >
              <Icon name="chevron-right" size={12} />
            </span>
          )}
          {entry.kind === "file" && <span className="sidebar__tree-indent" />}
          {entry.kind === "dir" ? (
            <Icon
              name={isExpanded ? "folder-open" : "folder"}
              size={14}
              className="sidebar__tree-glyph"
            />
          ) : (
            <Icon name="file" size={14} className="sidebar__tree-glyph" />
          )}
          <span className="sidebar__tree-name">{entry.name}</span>
          {liveDirty && (
            <span className="sidebar__tree-dirty tabular">
              {liveDirty.trim() || liveDirty}
            </span>
          )}
          {liveDiff && (
            <span className="sidebar__tree-diff tabular">
              +{liveDiff.additions} -{liveDiff.deletions}
            </span>
          )}
        </button>
      </Tooltip>
      {entry.kind === "dir" && (
        <input
          ref={uploadInputRef}
          type="file"
          multiple
          className="sidebar__hidden-upload"
          onChange={onUploadChange}
          aria-hidden
        />
      )}
      {isExpanded &&
        entry.kind === "dir" &&
        childEntries !== undefined &&
        childEntries !== null && (
          <TreeNodes
            repoName={repoName}
            path={fullPath}
            entries={childEntries.entries}
            dirtyExpand={dirtyExpand}
            depth={depth + 1}
            onError={onError}
            revealRequest={revealRequest}
          />
        )}
    </li>
  );
}

// ─── Git panel ──────────────────────────────────────────────────────

function GitPanel({ git }: { git: RepoGitSummary | null }) {
  if (!git) {
    return <div className="sidebar__muted">no git info yet</div>;
  }
  if (!git.branch && !git.last_commit) {
    return <div className="sidebar__muted">not a git repo</div>;
  }
  return (
    <div className="sidebar__git">
      {git.last_commit ? (
        <div className="sidebar__git-last">
          <div className="sidebar__git-age">
            last commit · {relativeAge(git.last_commit.committed_at)}
          </div>
          <div className="sidebar__git-subject">"{git.last_commit.subject}"</div>
        </div>
      ) : (
        <div className="sidebar__muted">no commits yet</div>
      )}
      <div className="sidebar__git-counts">
        uncommitted: <strong>{git.uncommitted_count}</strong>
        {git.untracked_count > 0 && <> ({git.untracked_count} untracked)</>}
      </div>
      {git.recent_commits.length > 1 && (
        <div className="sidebar__git-recent">
          <div className="sidebar__git-recent-label">recent</div>
          <ul>
            {git.recent_commits.slice(1).map((c) => (
              <RecentCommit key={c.sha} commit={c} />
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

function RecentCommit({ commit }: { commit: GitCommit }) {
  return (
    <Tooltip label={commit.subject}>
      <li className="sidebar__git-recent-item">
        <span className="sidebar__git-recent-age tabular">
          {relativeAge(commit.committed_at)}
        </span>
        <span className="sidebar__git-recent-subject">{commit.subject}</span>
      </li>
    </Tooltip>
  );
}

// ─── Session row (unchanged behaviour; reshuffled for subsection) ───

function buildSessionMenuItems({
  session,
  openTab,
  onRename,
  secretMenu,
  onUpdate,
  onDelete,
}: {
  session: SessionView;
  openTab: TabStore["openTab"];
  onRename: () => void;
  secretMenu: MenuItem;
  onUpdate: (patch: {
    label?: string | null;
    pinned?: boolean;
    color?: SessionColor | null;
  }) => void | Promise<void>;
  onDelete: () => void;
}): MenuItem[] {
  return [
    {
      kind: "item",
      id: "open-terminal",
      label: "Open terminal",
      onSelect: () => {
        openTab({ kind: "terminal", sessionId: session.id }, "top");
        appCommands.closeDrawer();
      },
    },
    {
      kind: "item",
      id: "open-timeline",
      label: "Open timeline",
      onSelect: () => {
        openTab({ kind: "timeline", sessionId: session.id }, "bottom");
        appCommands.closeDrawer();
      },
    },
    {
      kind: "item",
      id: "future-prompts",
      label: "Future prompts",
      onSelect: () => appCommands.openFuturePrompts({ sessionId: session.id }),
    },
    secretMenu,
    {
      kind: "item",
      id: "open-repo-diff",
      label: session.workspace ? "Open workspace diff" : "Open repo diff",
      onSelect: () => {
        openTab({
          kind: "diff",
          repo: session.repo,
          workspaceId: session.workspace?.id,
        });
        appCommands.closeDrawer();
      },
    },
    { kind: "separator" },
    { kind: "item", id: "rename", label: "Rename", onSelect: onRename },
    {
      kind: "item",
      id: "pin",
      label: session.pinned ? "Unpin" : "Pin to top",
      onSelect: () => void onUpdate({ pinned: !session.pinned }),
    },
    {
      kind: "submenu",
      id: "colour",
      label: "Colour",
      items: [
        {
          kind: "item",
          id: "colour-none",
          label: "None",
          onSelect: () => void onUpdate({ color: null }),
        },
        ...SESSION_COLORS.map<MenuItem>((color) => ({
          kind: "item",
          id: `colour-${color}`,
          label: color,
          onSelect: () => void onUpdate({ color }),
        })),
      ],
    },
    { kind: "separator" },
    {
      kind: "item",
      id: "delete",
      label: "Delete session",
      destructive: true,
      onSelect: onDelete,
    },
  ];
}

interface SessionRowProps {
  session: SessionView;
  selected: boolean;
  unread: boolean;
  onSelect: (id: string) => void;
  onDelete: (id: string) => void;
  onUpdate: (
    id: string,
    patch: {
      label?: string | null;
      pinned?: boolean;
      color?: SessionColor | null;
    },
  ) => void | Promise<void>;
}

function SessionRow({
  session: s,
  selected,
  unread,
  onSelect,
  onDelete,
  onUpdate,
}: SessionRowProps) {
  const [renaming, setRenaming] = useState(false);
  const openTab = useTabs((store) => store.openTab);
  const openCtx = useContextMenu((store) => store.open);
  const {
    secrets,
    grants,
    refreshSecrets,
    refreshGrants,
    enableGrant,
    revokeGrant,
  } = useSecretStore(
    useShallow((store) => ({
      secrets: store.secrets,
      grants: store.grantsBySession[s.id] ?? EMPTY_SECRET_GRANTS,
      refreshSecrets: store.refreshSecrets,
      refreshGrants: store.refreshGrants,
      enableGrant: store.enableGrant,
      revokeGrant: store.revokeGrant,
    })),
  );

  const selectThis = useCallback(() => onSelect(s.id), [onSelect, s.id]);
  const deleteThis = useCallback(() => onDelete(s.id), [onDelete, s.id]);
  const updateThis = useCallback(
    (patch: Parameters<SessionRowProps["onUpdate"]>[1]) => onUpdate(s.id, patch),
    [onUpdate, s.id],
  );
  const startRenaming = useCallback(() => setRenaming(true), []);
  const stopRenaming = useCallback(() => setRenaming(false), []);
  const openSecrets = useCallback(() => {
    openTab({ kind: "secrets", sessionId: s.id }, "top");
    appCommands.closeDrawer();
  }, [openTab, s.id]);
  useEffect(() => {
    void refreshSecrets().catch(() => undefined);
    void refreshGrants(s.id).catch(() => undefined);
  }, [refreshSecrets, refreshGrants, s.id]);
  const enableSecret = useCallback(
    (secretId: string, tool: "with-cred" | "aws", ttlSeconds: number) => {
      void enableGrant(s.id, secretId, tool, ttlSeconds).catch(() => undefined);
    },
    [enableGrant, s.id],
  );
  const revokeSecret = useCallback(
    (secretId: string, tool: "with-cred" | "aws") => {
      void revokeGrant(s.id, secretId, tool).catch(() => undefined);
    },
    [revokeGrant, s.id],
  );
  const secretMenu = useMemo(
    () =>
      buildSecretContextMenu({
        secrets,
        grants,
        onEnable: enableSecret,
        onRevoke: revokeSecret,
        onOpenManager: openSecrets,
      }),
    [enableSecret, grants, openSecrets, revokeSecret, secrets],
  );

  const menuItems = useMemo(
    () =>
      buildSessionMenuItems({
        session: s,
        openTab,
        onRename: startRenaming,
        secretMenu,
        onUpdate: updateThis,
        onDelete: deleteThis,
      }),
    [s, openTab, startRenaming, secretMenu, updateThis, deleteThis],
  );
  const buildMenuItems = useCallback(() => menuItems, [menuItems]);
  const { onContextMenu: onRowContextMenu, onKeyDown: onRowContextMenuKey } =
    useMemo(
      () => contextMenuTriggerProps(openCtx, buildMenuItems),
      [openCtx, buildMenuItems],
    );

  const sessionLabel = (() => {
    if (s.state === "dead") return "ended";
    if (s.state === "orphaned") return "orphaned";
    if (s.state === "deleted") return "—";
    if (!s.current_session_uuid) {
      return s.current_session_agent ? `${s.current_session_agent} starting` : "starting";
    }
    const agent = s.current_session_agent ?? "session";
    return `${agent} ${s.current_session_uuid.slice(0, 6)}`;
  })();

  const displayName =
    s.label && s.label.length > 0 ? s.label : s.id.slice(0, 8);

  // Session working_dir relative to the repo root — displayed on the
  // row so you can tell "this session runs in src/" vs "this one at
  // the root" at a glance. Empty means the session runs at the repo
  // root (the common case).
  const cwdHint = (() => {
    if (s.workspace?.kind === "worktree") {
      return s.workspace.branch_name ?? "isolated";
    }
    const wd = s.working_dir;
    if (!wd) return null;
    const idx = wd.indexOf(`/${s.repo}/`);
    if (idx === -1) return null;
    const rel = wd.slice(idx + s.repo.length + 2).replace(/\/+$/, "");
    return rel.length > 0 ? rel : null;
  })();
  const workspaceTone = s.workspace?.kind === "worktree" ? "workspace" : null;

  const rowClass = [
    "sidebar__row",
    s.color ? `sidebar__row--color-${s.color}` : null,
  ]
    .filter(Boolean)
    .join(" ");

  const stateIcon =
    s.state === "dead"
      ? "session-dead"
      : s.state === "orphaned"
        ? "session-orphan"
        : s.state === "deleted"
          ? "session-dead"
          : "session-live";
  const stateTone =
    s.state === "dead"
      ? "crit"
      : s.state === "orphaned"
        ? "atn"
        : s.state === "deleted"
          ? "mute"
          : "ok";

  const submitRename = useCallback(
    (value: string) => {
      const v = value.trim();
      void updateThis({ label: v.length === 0 ? null : v });
      setRenaming(false);
    },
    [updateThis],
  );

  return (
    <div className={rowClass}>
      {s.color && <span className="sidebar__color-accent" aria-hidden />}
      {renaming ? (
        <RenameInput
          initial={s.label ?? ""}
          onSubmit={submitRename}
          onCancel={stopRenaming}
        />
      ) : (
        <Tooltip label="Right-click for session actions">
          <button
            type="button"
            className={
              selected
                ? "sidebar__session sidebar__session--active"
                : "sidebar__session"
            }
            data-session-id={s.id}
            data-session-name={displayName}
            data-session-repo={s.repo}
            onClick={selectThis}
            onDoubleClick={startRenaming}
            onContextMenu={onRowContextMenu}
            onKeyDown={onRowContextMenuKey}
          >
            <span
              className={`sidebar__dot sidebar__dot--${s.state} sidebar__dot--tone-${stateTone}`}
              aria-hidden
            >
              <Icon name={stateIcon} size={12} />
            </span>
            <span className="sidebar__session-main">
              <span className="sidebar__session-id">
                {s.pinned && (
                  <span className="sidebar__pin-indicator" aria-label="pinned">
                    <Icon name="pin" size={12} />
                  </span>
                )}
                {displayName}
              </span>
              <span className="sidebar__session-meta tabular">
                {ageSince(s.created_at)} · {sessionLabel}
                {cwdHint && (
                  <>
                    {" · "}
                    <span
                      className={
                        workspaceTone
                          ? "sidebar__cwd sidebar__cwd--workspace"
                          : "sidebar__cwd"
                      }
                    >
                      {cwdHint}
                    </span>
                  </>
                )}
              </span>
            </span>
            {s.future_prompts_pending_count > 0 && (
              <span
                className="sidebar__future-prompts"
                data-testid="session-future-prompts-badge"
                aria-label={`${s.future_prompts_pending_count} queued future prompt${s.future_prompts_pending_count === 1 ? "" : "s"}`}
              >
                <Icon name="list-checks" size={12} />
                <span className="tabular">{s.future_prompts_pending_count}</span>
              </span>
            )}
            {unread && !selected && (
              <span
                className="sidebar__unread"
                aria-label="new activity since last view"
              >
                <Icon name="unread" size={12} />
              </span>
            )}
          </button>
        </Tooltip>
      )}
    </div>
  );
}

function RenameInput({
  initial,
  onSubmit,
  onCancel,
}: {
  initial: string;
  onSubmit: (value: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  const onFormSubmit = useCallback(
    (e: FormEvent) => {
      e.preventDefault();
      onSubmit(value);
    },
    [onSubmit, value],
  );
  const onInputChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => setValue(e.target.value),
    [],
  );
  const onInputKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onCancel();
      }
    },
    [onCancel],
  );
  const onInputBlur = useCallback(() => onSubmit(value), [onSubmit, value]);
  return (
    <form className="sidebar__rename" onSubmit={onFormSubmit}>
      <input
        type="text"
        className="sidebar__rename-input"
        value={value}
        onChange={onInputChange}
        autoFocus
        maxLength={100}
        placeholder="Session name (empty to clear)"
        aria-label="Session name"
        onKeyDown={onInputKeyDown}
        onBlur={onInputBlur}
      />
    </form>
  );
}

function NewRepoForm({
  onSubmit,
  onCancel,
}: {
  onSubmit: (form: { name: string; git_url: string }) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [gitUrl, setGitUrl] = useState("");
  const submit = useCallback(
    (e: FormEvent) => {
      e.preventDefault();
      if (!name.trim()) return;
      onSubmit({ name: name.trim(), git_url: gitUrl });
    },
    [name, gitUrl, onSubmit],
  );
  const cancelOnEscape = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === "Escape") onCancel();
    },
    [onCancel],
  );
  const onNameChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => setName(e.target.value),
    [],
  );
  const onGitUrlChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => setGitUrl(e.target.value),
    [],
  );
  return (
    <form className="sidebar__form" onSubmit={submit}>
      <input
        type="text"
        value={name}
        onChange={onNameChange}
        onKeyDown={cancelOnEscape}
        placeholder="repo name"
        autoFocus
        aria-label="repo name"
      />
      <input
        type="text"
        value={gitUrl}
        onChange={onGitUrlChange}
        onKeyDown={cancelOnEscape}
        placeholder="git url (optional)"
        aria-label="git url"
      />
      <div className="sidebar__form-actions">
        <button type="button" onClick={onCancel}>
          Cancel
        </button>
        <button type="submit">Create</button>
      </div>
    </form>
  );
}

function NewSessionForm({
  repoName,
  onSubmit,
  onCancel,
}: {
  repoName: string;
  onSubmit: (form: NewSessionFormValue) => void;
  onCancel: () => void;
}) {
  const [workingDir, setWorkingDir] = useState("");
  const [launchAgent, setLaunchAgent] = useState<AgentLaunchType | "">("");
  const [workspaceMode, setWorkspaceMode] =
    useState<NewSessionFormValue["workspace_mode"]>("isolated");
  const submit = useCallback(
    (e: FormEvent) => {
      e.preventDefault();
      onSubmit({
        working_dir: workspaceMode === "main" ? workingDir : "",
        launch_agent: launchAgent,
        workspace_mode: workspaceMode,
      });
    },
    [launchAgent, onSubmit, workingDir, workspaceMode],
  );
  const onInputChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => setWorkingDir(e.target.value),
    [],
  );
  const onAgentChange = useCallback(
    (e: React.ChangeEvent<HTMLSelectElement>) =>
      setLaunchAgent(e.target.value as AgentLaunchType | ""),
    [],
  );
  const onWorkspaceModeChange = useCallback(
    (e: React.ChangeEvent<HTMLSelectElement>) =>
      setWorkspaceMode(e.target.value as NewSessionFormValue["workspace_mode"]),
    [],
  );
  const onInputKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === "Escape") onCancel();
    },
    [onCancel],
  );
  return (
    <form className="sidebar__form" onSubmit={submit}>
      <input
        type="text"
        value={workingDir}
        onChange={onInputChange}
        onKeyDown={onInputKeyDown}
        placeholder={
          workspaceMode === "main"
            ? `working dir (default: repos/${repoName})`
            : "working dir is the isolated workspace root"
        }
        disabled={workspaceMode !== "main"}
        autoFocus
        aria-label="working directory"
      />
      <select
        value={workspaceMode}
        onChange={onWorkspaceModeChange}
        aria-label="workspace mode"
      >
        <option value="isolated">Isolated worktree</option>
        <option value="main">Main working tree</option>
      </select>
      <select
        value={launchAgent}
        onChange={onAgentChange}
        aria-label="launch agent"
      >
        <option value="">Shell only</option>
        <option value="claude">Launch Claude</option>
        <option value="codex">Launch Codex</option>
      </select>
      <div className="sidebar__form-actions">
        <button type="button" onClick={onCancel}>
          Cancel
        </button>
        <button type="submit">Spawn</button>
      </div>
    </form>
  );
}

function ageSince(iso: string): string {
  return relativeAge(iso);
}

function relativeAge(iso: string): string {
  const then = new Date(iso).getTime();
  if (!Number.isFinite(then)) return "—";
  const diff = Date.now() - then;
  const s = Math.round(diff / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h`;
  const d = Math.round(h / 24);
  return `${d}d`;
}

function messageOf(err: unknown): string {
  if (err instanceof ApiError) return err.message;
  if (err instanceof Error) return err.message;
  return "unknown error";
}
