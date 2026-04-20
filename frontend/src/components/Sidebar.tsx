// Sidebar: global library at the top, then repo-centric navigation
// (Sessions / Files / Git) with compact repo status badges. Repos are
// still the main navigation axis for workspace state; prompts and
// references are global because they are cross-repo user tools.

import {
  type FormEvent,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useShallow } from "zustand/react/shallow";

import type {
  DirEntryView,
  GitCommit,
  GitStatus,
  RepoView,
  SessionColor,
  SessionView,
} from "../api/types";
import { SESSION_COLORS } from "../api/types";
import { ApiError, stageRepoPath, uploadRepoFile } from "../api/client";
import { appCommands, useAppCommand } from "../state/AppCommands";
import { useSessions } from "../state/SessionStore";
import { dirtyAncestors, stalenessFor, useRepos } from "../state/RepoStore";
import type { TabStore } from "../state/TabStore";
import { useTabs } from "../state/TabStore";
import { Icon } from "../icons";
import { Tooltip } from "./ui";
import type { MenuItem } from "./common/ContextMenu";
import {
  contextMenuHandler,
  useContextMenu,
} from "./common/ContextMenu";
import { ConfirmDialog } from "./common/ConfirmDialog";
import { LibraryPanel } from "./LibraryPanel";
import { StatsStrip } from "./StatsStrip";
import "./Sidebar.css";
import "./LibrarySection.css";

export function Sidebar() {
  const {
    sessions,
    repos,
    selectedSessionId,
    selectSession,
    createSession,
    deleteSession,
    updateSession,
    createRepo,
    isUnread,
  } = useSessions(
    useShallow((store) => ({
      sessions: store.sessions,
      repos: store.repos,
      selectedSessionId: store.selectedSessionId,
      selectSession: store.selectSession,
      createSession: store.createSession,
      deleteSession: store.deleteSession,
      updateSession: store.updateSession,
      createRepo: store.createRepo,
      isUnread: store.isUnread,
    })),
  );

  const openTab = useTabs((store) => store.openTab);

  const grouped = useMemo(() => groupByRepo(sessions, repos), [sessions, repos]);

  // Opening a session's work area: terminal top + timeline bottom.
  // Called directly from the click handler (no useEffect on selected
  // session) so sidebar interaction doesn't fight file/tab
  // activation through the global tab state.
  const openSessionTabs = (id: string) => {
    selectSession(id);
    openTab({ kind: "terminal", sessionId: id }, "top");
    openTab({ kind: "timeline", sessionId: id }, "bottom");
    appCommands.closeDrawer();
  };
  const [expanded, setExpanded] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(grouped.map((g) => [g.name, true])),
  );
  const [newRepoOpen, setNewRepoOpen] = useState(false);
  const [newSessionFor, setNewSessionFor] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [revealRequest, setRevealRequest] = useState<{
    repo: string;
    path: string;
    nonce: number;
  } | null>(null);

  useAppCommand("reveal-file", ({ repo, path }) => {
    setExpanded((prev) => ({ ...prev, [repo]: true }));
    setRevealRequest({ repo, path, nonce: Date.now() });
  });

  const repoAnchorsRef = useRef<Map<string, HTMLLIElement>>(new Map());
  const registerRepoAnchor = (name: string, el: HTMLLIElement | null) => {
    if (el) repoAnchorsRef.current.set(name, el);
    else repoAnchorsRef.current.delete(name);
  };

  useAppCommand("reveal-repo", ({ repo }) => {
    setExpanded((prev) => ({ ...prev, [repo]: true }));
    const el = repoAnchorsRef.current.get(repo);
    if (el) {
      requestAnimationFrame(() =>
        el.scrollIntoView({ block: "start", behavior: "smooth" }),
      );
    }
  });

  const toggleRepo = (name: string) =>
    setExpanded((prev) => ({ ...prev, [name]: !prev[name] }));

  const onCreateRepo = async (form: { name: string; git_url: string }) => {
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
  };

  const onCreateSession = async (
    repoName: string,
    form: { working_dir: string },
  ) => {
    setFormError(null);
    try {
      await createSession({
        repo: repoName,
        working_dir: form.working_dir.trim() || undefined,
      });
      setNewSessionFor(null);
    } catch (err) {
      setFormError(messageOf(err));
    }
  };

  const requestDelete = (id: string) => setPendingDeleteId(id);
  const confirmDelete = async () => {
    const id = pendingDeleteId;
    if (!id) return;
    setPendingDeleteId(null);
    try {
      await deleteSession(id);
    } catch (err) {
      setFormError(messageOf(err));
    }
  };

  const onUpdateSession = async (
    id: string,
    patch: Parameters<typeof updateSession>[1],
  ) => {
    setFormError(null);
    try {
      await updateSession(id, patch);
    } catch (err) {
      setFormError(messageOf(err));
    }
  };

  return (
    <div className="sidebar">
      <div className="sidebar__header">
        <span className="sidebar__logo">sulion</span>
        <Tooltip label="New repo">
          <button
            type="button"
            className="sidebar__icon-button"
            onClick={() => {
              setNewRepoOpen((v) => !v);
              setNewSessionFor(null);
            }}
            aria-label="New repo"
          >
            <Icon name="plus" size={14} />
          </button>
        </Tooltip>
      </div>

      {newRepoOpen && (
        <NewRepoForm
          onSubmit={onCreateRepo}
          onCancel={() => setNewRepoOpen(false)}
        />
      )}

      {formError && <div className="sidebar__error">{formError}</div>}

      <LibraryPanel />

      {grouped.length === 0 && <div className="sidebar__muted">No repos yet.</div>}

      <ul className="sidebar__tree">
        {grouped.map((group) => (
          <RepoGroup
            key={group.name}
            group={group}
            expanded={expanded[group.name] ?? true}
            onToggle={() => toggleRepo(group.name)}
            selectedSessionId={selectedSessionId}
            onSelectSession={openSessionTabs}
            onRequestDelete={requestDelete}
            onUpdateSession={onUpdateSession}
            onNewSession={() =>
              setNewSessionFor((v) => (v === group.name ? null : group.name))
            }
            newSessionOpen={newSessionFor === group.name}
            onNewSessionSubmit={(form) => onCreateSession(group.name, form)}
            onNewSessionCancel={() => setNewSessionFor(null)}
            isUnread={isUnread}
            onError={setFormError}
            revealRequest={
              revealRequest?.repo === group.name ? revealRequest : null
            }
            anchorRef={(el) => registerRepoAnchor(group.name, el)}
          />
        ))}
      </ul>
      {pendingDeleteId && (
        <ConfirmDialog
          title="Delete session?"
          message="This terminates the shell and marks the session deleted. Any running command loses its process."
          confirmLabel="Delete"
          destructive
          onConfirm={confirmDelete}
          onCancel={() => setPendingDeleteId(null)}
        />
      )}
      <div className="sidebar__spacer" />
      <StatsStrip />
    </div>
  );
}

interface RepoGroupData {
  name: string;
  exists: boolean;
  sessions: SessionView[];
}

function groupByRepo(sessions: SessionView[], repos: RepoView[]): RepoGroupData[] {
  const byName = new Map<string, RepoGroupData>();
  for (const r of repos) {
    byName.set(r.name, { name: r.name, exists: true, sessions: [] });
  }
  for (const s of sessions) {
    if (!byName.has(s.repo)) {
      byName.set(s.repo, { name: s.repo, exists: false, sessions: [] });
    }
    byName.get(s.repo)!.sessions.push(s);
  }
  for (const g of byName.values()) {
    g.sessions.sort(sessionCompare);
  }
  return Array.from(byName.values()).sort((a, b) => a.name.localeCompare(b.name));
}

function sessionCompare(a: SessionView, b: SessionView): number {
  const ap = a.pinned ? 1 : 0;
  const bp = b.pinned ? 1 : 0;
  if (ap !== bp) return bp - ap;
  return new Date(b.created_at).getTime() - new Date(a.created_at).getTime();
}

// ─── Repo group ─────────────────────────────────────────────────────

function RepoGroup({
  group,
  expanded,
  onToggle,
  selectedSessionId,
  onSelectSession,
  onRequestDelete,
  onUpdateSession,
  onNewSession,
  newSessionOpen,
  onNewSessionSubmit,
  onNewSessionCancel,
  isUnread,
  onError,
  revealRequest,
  anchorRef,
}: {
  group: RepoGroupData;
  expanded: boolean;
  onToggle: () => void;
  selectedSessionId: string | null;
  onSelectSession: (id: string) => void;
  onRequestDelete: (id: string) => void;
  onUpdateSession: (
    id: string,
    patch: {
      label?: string | null;
      pinned?: boolean;
      color?: SessionColor | null;
    },
  ) => void | Promise<void>;
  onNewSession: () => void;
  newSessionOpen: boolean;
  onNewSessionSubmit: (form: { working_dir: string }) => void;
  onNewSessionCancel: () => void;
  isUnread: (sessionId: string, lastEventAt: string | null) => boolean;
  onError: (message: string | null) => void;
  revealRequest: { repo: string; path: string; nonce: number } | null;
  anchorRef?: (el: HTMLLIElement | null) => void;
}) {
  const { setExpanded, repoState } = useRepos(
    useShallow((store) => ({
      setExpanded: store.setExpanded,
      repoState: store.repos[group.name],
    })),
  );
  const git = repoState?.git ?? null;
  const [subOpen, setSubOpen] = useState({
    sessions: true,
    files: false,
    gitSection: true,
  });

  useEffect(() => {
    setExpanded(group.name, expanded);
  }, [group.name, expanded, setExpanded]);

  useEffect(() => {
    if (!revealRequest) return;
    setSubOpen((prev) => ({ ...prev, files: true }));
  }, [revealRequest]);

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

  return (
    <li className="sidebar__group" ref={anchorRef}>
      <div className="sidebar__group-header">
        <button
          type="button"
          className="sidebar__group-toggle"
          onClick={onToggle}
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
            onToggle={() =>
              setSubOpen((p) => ({ ...p, sessions: !p.sessions }))
            }
            count={group.sessions.length}
            rightSlot={
              <Tooltip label={`New session in ${group.name}`}>
                <button
                  type="button"
                  className="sidebar__icon-button"
                  aria-label={`New session in ${group.name}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    onNewSession();
                  }}
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
                onSubmit={onNewSessionSubmit}
                onCancel={onNewSessionCancel}
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
                onSelect={() => onSelectSession(s.id)}
                onDelete={() => onRequestDelete(s.id)}
                onUpdate={(patch) => onUpdateSession(s.id, patch)}
              />
            ))}
          </Subsection>

          <Subsection
            label="Files"
            open={subOpen.files}
            onToggle={() => setSubOpen((p) => ({ ...p, files: !p.files }))}
          >
            {subOpen.files && (
              <FileTree
                repoName={group.name}
                onError={onError}
                revealRequest={revealRequest}
              />
            )}
          </Subsection>

          <Subsection
            label="Git"
            open={subOpen.gitSection}
            onToggle={() =>
              setSubOpen((p) => ({ ...p, gitSection: !p.gitSection }))
            }
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
  git: GitStatus;
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

// ─── File tree ──────────────────────────────────────────────────────

function FileTree({
  repoName,
  onError,
  revealRequest,
}: {
  repoName: string;
  onError: (message: string | null) => void;
  revealRequest: { repo: string; path: string; nonce: number } | null;
}) {
  const { state, loadDir, setShowAll, expandPath } = useRepos(
    useShallow((store) => ({
      state: store.repos[repoName],
      loadDir: store.loadDir,
      setShowAll: store.setShowAll,
      expandPath: store.expandPath,
    })),
  );

  useEffect(() => {
    loadDir(repoName, "");
  }, [loadDir, repoName]);

  useEffect(() => {
    if (!revealRequest) return;
    expandPath(repoName, revealRequest.path);
  }, [expandPath, repoName, revealRequest]);

  const dirtyExpand = useMemo(
    () => dirtyAncestors(state?.git?.dirty_by_path ?? {}),
    [state?.git?.dirty_by_path],
  );

  const root = state?.tree[""];
  if (root === undefined) {
    return <div className="sidebar__muted">loading…</div>;
  }
  if (root === null) {
    return <div className="sidebar__muted">loading…</div>;
  }

  return (
    <div className="sidebar__tree-body">
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
          onChange={(e) => setShowAll(repoName, e.target.checked)}
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
  useEffect(() => {
    if (!isRevealTarget) return;
    rowRef.current?.scrollIntoView({ block: "center" });
  }, [isRevealTarget, revealRequest?.nonce]);

  const onClickRow = () => {
    if (entry.kind === "dir") {
      toggleDir(repoName, fullPath, isExpanded);
    } else {
      appCommands.openFile({ repo: repoName, path: fullPath });
    }
  };

  const openFileTab = () => appCommands.openFile({ repo: repoName, path: fullPath });

  const copyPath = (variant: "absolute" | "relative") => {
    const text = variant === "absolute"
      ? `/home/dev/repos/${repoName}/${fullPath}`
      : fullPath;
    void navigator.clipboard?.writeText(text).catch(() => {
      /* ignore — HTTP deploys lack permission */
    });
  };

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

  const onUploadChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
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
  };

  const dropHandlers =
    entry.kind === "dir"
      ? {
          onDragOver: (ev: React.DragEvent) => {
            if (ev.dataTransfer.types.includes("Files")) {
              ev.preventDefault();
              setDragOver(true);
            }
          },
          onDragLeave: () => setDragOver(false),
          onDrop: async (ev: React.DragEvent) => {
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
        }
      : {};

  const childEntries = state?.tree[fullPath];

  const tooltip = entry.dirty ? `${entry.dirty.trim()} ${fullPath}` : fullPath;
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
            (entry.dirty ? " sidebar__tree-row--dirty" : "") +
            (isRevealTarget ? " sidebar__tree-row--revealed" : "")
          }
          // eslint-disable-next-line local/no-inline-styles -- depth is per-row; can't be expressed as a finite class set
          style={{ paddingLeft: 4 + depth * 12 }}
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
          {entry.dirty && (
            <span className="sidebar__tree-dirty tabular">
              {entry.dirty.trim() || entry.dirty}
            </span>
          )}
          {entry.diff && (
            <span className="sidebar__tree-diff tabular">
              +{entry.diff.additions} -{entry.diff.deletions}
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

function GitPanel({ git }: { git: GitStatus | null }) {
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
  onUpdate,
  onDelete,
}: {
  session: SessionView;
  openTab: TabStore["openTab"];
  onRename: () => void;
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
      onSelect: () => openTab({ kind: "terminal", sessionId: session.id }, "top"),
    },
    {
      kind: "item",
      id: "open-timeline",
      label: "Open timeline",
      onSelect: () => openTab({ kind: "timeline", sessionId: session.id }, "bottom"),
    },
    {
      kind: "item",
      id: "open-repo-diff",
      label: "Open repo diff",
      onSelect: () => openTab({ kind: "diff", repo: session.repo }),
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

function SessionRow({
  session: s,
  selected,
  unread,
  onSelect,
  onDelete,
  onUpdate,
}: {
  session: SessionView;
  selected: boolean;
  unread: boolean;
  onSelect: () => void;
  onDelete: () => void;
  onUpdate: (patch: {
    label?: string | null;
    pinned?: boolean;
    color?: SessionColor | null;
  }) => void | Promise<void>;
}) {
  const [renaming, setRenaming] = useState(false);
  const openTab = useTabs((store) => store.openTab);
  const openCtx = useContextMenu((store) => store.open);

  const menuItems = buildSessionMenuItems({
    session: s,
    openTab,
    onRename: () => setRenaming(true),
    onUpdate,
    onDelete,
  });
  const onRowContextMenu = contextMenuHandler(openCtx, () => menuItems);

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
    const wd = s.working_dir;
    if (!wd) return null;
    const idx = wd.indexOf(`/${s.repo}/`);
    if (idx === -1) return null;
    const rel = wd.slice(idx + s.repo.length + 2).replace(/\/+$/, "");
    return rel.length > 0 ? rel : null;
  })();

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

  return (
    <div className={rowClass} onContextMenu={onRowContextMenu}>
      {s.color && <span className="sidebar__color-accent" aria-hidden />}
      {renaming ? (
        <RenameInput
          initial={s.label ?? ""}
          onSubmit={(value) => {
            const v = value.trim();
            void onUpdate({ label: v.length === 0 ? null : v });
            setRenaming(false);
          }}
          onCancel={() => setRenaming(false)}
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
            onClick={onSelect}
            onDoubleClick={() => setRenaming(true)}
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
                    <span className="sidebar__cwd">{cwdHint}</span>
                  </>
                )}
              </span>
            </span>
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
  return (
    <form
      className="sidebar__rename"
      onSubmit={(e) => {
        e.preventDefault();
        onSubmit(value);
      }}
    >
      <input
        type="text"
        className="sidebar__rename-input"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        autoFocus
        maxLength={100}
        placeholder="Session name (empty to clear)"
        aria-label="Session name"
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            onCancel();
          }
        }}
        onBlur={() => onSubmit(value)}
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
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;
    onSubmit({ name: name.trim(), git_url: gitUrl });
  };
  return (
    <form
      className="sidebar__form"
      onSubmit={submit}
      onKeyDown={(e) => {
        if (e.key === "Escape") onCancel();
      }}
    >
      <input
        type="text"
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="repo name"
        autoFocus
        aria-label="repo name"
      />
      <input
        type="text"
        value={gitUrl}
        onChange={(e) => setGitUrl(e.target.value)}
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
  onSubmit: (form: { working_dir: string }) => void;
  onCancel: () => void;
}) {
  const [workingDir, setWorkingDir] = useState("");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    onSubmit({ working_dir: workingDir });
  };
  return (
    <form
      className="sidebar__form"
      onSubmit={submit}
      onKeyDown={(e) => {
        if (e.key === "Escape") onCancel();
      }}
    >
      <input
        type="text"
        value={workingDir}
        onChange={(e) => setWorkingDir(e.target.value)}
        placeholder={`working dir (default: repos/${repoName})`}
        autoFocus
        aria-label="working directory"
      />
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
