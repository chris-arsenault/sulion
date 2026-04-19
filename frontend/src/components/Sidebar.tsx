// Sidebar: repo tree + grouped session list, with inline forms for
// creating repos and sessions and a delete affordance on each session.

import { type FormEvent, useMemo, useState } from "react";

import type { RepoView, SessionView } from "../api/types";
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
  return Array.from(byName.values()).sort((a, b) => a.name.localeCompare(b.name));
}

function SessionRow({
  session: s,
  selected,
  unread,
  onSelect,
  onDelete,
}: {
  session: SessionView;
  selected: boolean;
  unread: boolean;
  onSelect: () => void;
  onDelete: () => void;
}) {
  const claudeLabel = (() => {
    if (s.state === "dead") return "ended";
    if (s.state === "orphaned") return "orphaned";
    if (s.state === "deleted") return "—";
    if (!s.current_claude_session_uuid) return "claude starting";
    return `claude ${s.current_claude_session_uuid.slice(0, 6)}`;
  })();

  return (
    <li className="sidebar__row">
      <button
        type="button"
        className={
          selected
            ? "sidebar__session sidebar__session--active"
            : "sidebar__session"
        }
        onClick={onSelect}
      >
        <span className={`sidebar__dot sidebar__dot--${s.state}`} />
        <span className="sidebar__session-main">
          <span className="sidebar__session-id">{s.id.slice(0, 8)}</span>
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
    </li>
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
