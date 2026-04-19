// Sidebar: repo tree + grouped session list, with inline forms for
// creating repos and sessions and a delete affordance on each session.

import {
  type FormEvent,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import type {
  RepoView,
  SessionColor,
  SessionView,
} from "../api/types";
import { SESSION_COLORS } from "../api/types";
import { ApiError } from "../api/client";
import { useSessions } from "../state/SessionStore";
import { ConfirmDialog } from "./common/ConfirmDialog";
import "./Sidebar.css";

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
  } = useSessions();

  const grouped = useMemo(() => groupByRepo(sessions, repos), [sessions, repos]);
  const [expanded, setExpanded] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(grouped.map((g) => [g.name, true])),
  );
  const [newRepoOpen, setNewRepoOpen] = useState(false);
  const [newSessionFor, setNewSessionFor] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);

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

  const requestDelete = (id: string) => {
    setPendingDeleteId(id);
  };

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
        <span className="sidebar__logo">shuttlecraft</span>
        <button
          type="button"
          className="sidebar__icon-button"
          onClick={() => {
            setNewRepoOpen((v) => !v);
            setNewSessionFor(null);
          }}
          title="New repo"
          aria-label="New repo"
        >
          +
        </button>
      </div>

      {newRepoOpen && (
        <NewRepoForm
          onSubmit={onCreateRepo}
          onCancel={() => setNewRepoOpen(false)}
        />
      )}

      {formError && <div className="sidebar__error">{formError}</div>}

      {grouped.length === 0 && <div className="sidebar__muted">No repos yet.</div>}

      <ul className="sidebar__tree">
        {grouped.map((group) => (
          <li key={group.name} className="sidebar__group">
            <div className="sidebar__group-header">
              <button
                type="button"
                className="sidebar__group-toggle"
                onClick={() => toggleRepo(group.name)}
              >
                <span
                  className={
                    (expanded[group.name] ?? true)
                      ? "sidebar__chevron sidebar__chevron--open"
                      : "sidebar__chevron"
                  }
                >
                  ▸
                </span>
                <span className="sidebar__group-name">{group.name}</span>
                <span className="sidebar__group-count">{group.sessions.length}</span>
              </button>
              <button
                type="button"
                className="sidebar__icon-button"
                title={`New session in ${group.name}`}
                aria-label={`New session in ${group.name}`}
                onClick={() => {
                  setNewSessionFor(group.name === newSessionFor ? null : group.name);
                  setNewRepoOpen(false);
                }}
                disabled={!group.exists}
              >
                +
              </button>
            </div>

            {newSessionFor === group.name && (
              <NewSessionForm
                repoName={group.name}
                onSubmit={(form) => onCreateSession(group.name, form)}
                onCancel={() => setNewSessionFor(null)}
              />
            )}

            {(expanded[group.name] ?? true) && (
              <ul className="sidebar__list">
                {group.sessions.length === 0 && (
                  <li className="sidebar__muted">— no sessions —</li>
                )}
                {group.sessions.map((s) => (
                  <SessionRow
                    key={s.id}
                    session={s}
                    selected={s.id === selectedSessionId}
                    unread={isUnread(s.id, s.last_event_at)}
                    onSelect={() => selectSession(s.id)}
                    onDelete={() => requestDelete(s.id)}
                    onUpdate={(patch) => onUpdateSession(s.id, patch)}
                  />
                ))}
              </ul>
            )}
          </li>
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
    </div>
  );
}

interface RepoGroup {
  name: string;
  /** True if the repo exists in the `/api/repos` list (directory present). */
  exists: boolean;
  sessions: SessionView[];
}

function groupByRepo(sessions: SessionView[], repos: RepoView[]): RepoGroup[] {
  const byName = new Map<string, RepoGroup>();
  for (const r of repos) {
    byName.set(r.name, { name: r.name, exists: true, sessions: [] });
  }
  for (const s of sessions) {
    if (!byName.has(s.repo)) {
      byName.set(s.repo, { name: s.repo, exists: false, sessions: [] });
    }
    byName.get(s.repo)!.sessions.push(s);
  }
  // Pinned sessions float to the top of each repo group; otherwise
  // newest-first matches the server ordering and stays stable under
  // optimistic local pin toggles.
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

interface UpdatePatch {
  label?: string | null;
  pinned?: boolean;
  color?: SessionColor | null;
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
  onUpdate: (patch: UpdatePatch) => void | Promise<void>;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [renaming, setRenaming] = useState(false);

  const claudeLabel = (() => {
    if (s.state === "dead") return "ended";
    if (s.state === "orphaned") return "orphaned";
    if (s.state === "deleted") return "—";
    if (!s.current_claude_session_uuid) return "claude starting";
    return `claude ${s.current_claude_session_uuid.slice(0, 6)}`;
  })();

  const displayName =
    s.label && s.label.length > 0 ? s.label : s.id.slice(0, 8);

  const rowClass = [
    "sidebar__row",
    s.color ? `sidebar__row--color-${s.color}` : null,
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <li className={rowClass}>
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
          <span className={`sidebar__dot sidebar__dot--${s.state}`} />
          <span className="sidebar__session-main">
            <span className="sidebar__session-id">
              {s.pinned && (
                <span
                  className="sidebar__pin-indicator"
                  aria-label="pinned"
                  title="pinned"
                >
                  ★
                </span>
              )}
              {displayName}
            </span>
            <span className="sidebar__session-meta">
              {ageSince(s.created_at)} · {claudeLabel}
            </span>
          </span>
          {unread && !selected && (
            <span
              className="sidebar__unread"
              aria-label="new activity since last view"
              title="New events since you last viewed this session"
            />
          )}
        </button>
      )}
      {!renaming && (
        <div className="sidebar__row-actions">
          <button
            type="button"
            className="sidebar__menu-button"
            onClick={(e) => {
              e.stopPropagation();
              setMenuOpen((v) => !v);
            }}
            title="Session options"
            aria-label="Session options"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
          >
            ⋯
          </button>
          {menuOpen && (
            <SessionMenu
              session={s}
              onClose={() => setMenuOpen(false)}
              onRename={() => {
                setRenaming(true);
                setMenuOpen(false);
              }}
              onUpdate={onUpdate}
            />
          )}
          <button
            type="button"
            className="sidebar__delete"
            onClick={(e) => {
              e.stopPropagation();
              onDelete();
            }}
            title="Delete session"
            aria-label="Delete session"
          >
            ×
          </button>
        </div>
      )}
    </li>
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

function SessionMenu({
  session: s,
  onClose,
  onRename,
  onUpdate,
}: {
  session: SessionView;
  onClose: () => void;
  onRename: () => void;
  onUpdate: (patch: UpdatePatch) => void | Promise<void>;
}) {
  const menuRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const handleDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("mousedown", handleDown);
    document.addEventListener("keydown", handleKey);
    return () => {
      document.removeEventListener("mousedown", handleDown);
      document.removeEventListener("keydown", handleKey);
    };
  }, [onClose]);

  return (
    <div className="sidebar__menu" role="menu" ref={menuRef}>
      <button
        type="button"
        role="menuitem"
        className="sidebar__menu-item"
        onClick={onRename}
      >
        Rename
      </button>
      <button
        type="button"
        role="menuitem"
        className="sidebar__menu-item"
        onClick={() => {
          void onUpdate({ pinned: !s.pinned });
          onClose();
        }}
      >
        {s.pinned ? "Unpin" : "Pin to top"}
      </button>
      <div className="sidebar__menu-divider" />
      <div className="sidebar__menu-section">Colour</div>
      <div className="sidebar__colors" role="group" aria-label="Session colour">
        <button
          type="button"
          className={
            s.color == null
              ? "sidebar__swatch sidebar__swatch--none sidebar__swatch--selected"
              : "sidebar__swatch sidebar__swatch--none"
          }
          aria-label="No colour"
          title="No colour"
          onClick={() => {
            void onUpdate({ color: null });
            onClose();
          }}
        >
          ∅
        </button>
        {SESSION_COLORS.map((c) => (
          <button
            key={c}
            type="button"
            className={
              s.color === c
                ? `sidebar__swatch sidebar__swatch--${c} sidebar__swatch--selected`
                : `sidebar__swatch sidebar__swatch--${c}`
            }
            aria-label={`Colour ${c}`}
            title={c}
            onClick={() => {
              void onUpdate({ color: c });
              onClose();
            }}
          />
        ))}
      </div>
    </div>
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
    <form className="sidebar__form" onSubmit={submit} onKeyDown={(e) => {
      if (e.key === "Escape") onCancel();
    }}>
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
    <form className="sidebar__form" onSubmit={submit} onKeyDown={(e) => {
      if (e.key === "Escape") onCancel();
    }}>
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
  const then = new Date(iso).getTime();
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
