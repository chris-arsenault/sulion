import {
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";

import {
  createFuturePrompt,
  deleteFuturePrompt,
  listFuturePrompts,
  updateFuturePrompt,
} from "../api/client";
import type { FuturePromptEntry } from "../api/types";
import { Icon } from "../icons";
import { appCommands } from "../state/AppCommands";
import { useSessions } from "../state/SessionStore";
import { Overlay } from "./ui";
import "./FuturePromptsModal.css";

export function FuturePromptsModal({
  open,
  sessionId,
  onClose,
}: {
  open: boolean;
  sessionId: string | null;
  onClose: () => void;
}) {
  const session = useSessions((store) =>
    sessionId ? store.sessions.find((item) => item.id === sessionId) ?? null : null,
  );
  const [sessionUuid, setSessionUuid] = useState<string | null>(null);
  const [sessionAgent, setSessionAgent] = useState<string | null>(null);
  const [prompts, setPrompts] = useState<FuturePromptEntry[]>([]);
  const [draft, setDraft] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingText, setEditingText] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);
  const [createBusy, setCreateBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!open || !sessionId) return;
    try {
      const resp = await listFuturePrompts(sessionId);
      setSessionUuid(resp.session_uuid);
      setSessionAgent(resp.session_agent);
      setPrompts(resp.prompts);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "future prompt load failed");
    }
  }, [open, sessionId]);

  useEffect(() => {
    if (!open) {
      setSessionUuid(null);
      setSessionAgent(null);
      setPrompts([]);
      setDraft("");
      setEditingId(null);
      setEditingText("");
      setBusyId(null);
      setCreateBusy(false);
      setError(null);
      return;
    }
    void load();
  }, [open, load]);

  const canCreate = Boolean(
    sessionId &&
      session?.state === "live" &&
      sessionUuid &&
      draft.trim().length > 0,
  );

  const createOne = useCallback(async () => {
    if (!sessionId || !canCreate) return;
    setCreateBusy(true);
    try {
      const created = await createFuturePrompt(sessionId, { text: draft });
      setPrompts((prev) => sortPrompts([...prev, created]));
      setDraft("");
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "future prompt create failed");
    } finally {
      setCreateBusy(false);
    }
  }, [sessionId, canCreate, draft]);

  const startEdit = useCallback((entry: FuturePromptEntry) => {
    setEditingId(entry.id);
    setEditingText(entry.text);
  }, []);

  const cancelEdit = useCallback(() => {
    setEditingId(null);
    setEditingText("");
  }, []);

  const saveEdit = useCallback(async () => {
    if (!sessionId || !editingId || editingText.trim().length === 0) return;
    setBusyId(editingId);
    try {
      const updated = await updateFuturePrompt(sessionId, editingId, {
        text: editingText,
      });
      setPrompts((prev) =>
        sortPrompts(prev.map((entry) => (entry.id === updated.id ? updated : entry))),
      );
      setEditingId(null);
      setEditingText("");
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "future prompt save failed");
    } finally {
      setBusyId(null);
    }
  }, [sessionId, editingId, editingText]);

  const sendPrompt = useCallback(
    async (entry: FuturePromptEntry) => {
      if (!sessionId || session?.state !== "live") return;
      setBusyId(entry.id);
      try {
        appCommands.injectTerminal({ sessionId, text: entry.text });
        const updated = await updateFuturePrompt(sessionId, entry.id, {
          state: "sent",
        });
        setPrompts((prev) =>
          sortPrompts(prev.map((item) => (item.id === updated.id ? updated : item))),
        );
        setError(null);
        // Hand the caret off to the terminal: close the modal so the
        // focus that `inject-terminal` placed on the xterm input isn't
        // fighting the overlay's focus trap.
        onClose();
      } catch (err) {
        setError(err instanceof Error ? err.message : "future prompt send failed");
      } finally {
        setBusyId(null);
      }
    },
    [sessionId, session?.state, onClose],
  );

  const removePrompt = useCallback(
    async (entry: FuturePromptEntry) => {
      if (!sessionId) return;
      setBusyId(entry.id);
      try {
        await deleteFuturePrompt(sessionId, entry.id);
        setPrompts((prev) => prev.filter((item) => item.id !== entry.id));
        if (editingId === entry.id) {
          setEditingId(null);
          setEditingText("");
        }
        setError(null);
      } catch (err) {
        setError(err instanceof Error ? err.message : "future prompt delete failed");
      } finally {
        setBusyId(null);
      }
    },
    [sessionId, editingId],
  );

  const copyPrompt = useCallback(async (entry: FuturePromptEntry) => {
    try {
      await navigator.clipboard.writeText(entry.text);
    } catch {
      // Clipboard permissions may be unavailable.
    }
  }, []);

  const pending = useMemo(
    () => prompts.filter((entry) => entry.state === "pending"),
    [prompts],
  );
  const sent = useMemo(
    () => prompts.filter((entry) => entry.state === "sent"),
    [prompts],
  );

  const displayName = session
    ? session.label?.trim() || session.id.slice(0, 8)
    : "session";
  const subtitle = sessionUuid
    ? `${sessionAgent ?? "session"} ${sessionUuid.slice(0, 8)}`
    : "no correlated invocation";

  const footer = session ? (
    <div className="fp__footer">
      <span className={`fp__session-state fp__session-state--${session.state}`}>
        {session.state}
      </span>
      <span className="fp__footer-cwd">{session.repo}</span>
    </div>
  ) : undefined;

  return (
    <Overlay
      open={open && Boolean(sessionId)}
      onClose={onClose}
      modal
      width="min(92vw, 920px)"
      maxHeight="80vh"
      title="Future Prompts"
      subtitle={`${displayName} · ${subtitle}`}
      leading={<Icon name="terminal" size={14} />}
      footer={footer}
    >
      <div className="fp">
        <div className="fp__composer">
          <textarea
            className="fp__composer-input"
            placeholder={
              sessionUuid
                ? "Queue the next thing you want to say in this terminal…"
                : "This terminal has no correlated invocation yet."
            }
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            disabled={!sessionUuid || session?.state !== "live" || createBusy}
            rows={4}
          />
          <div className="fp__composer-actions">
            <button
              type="button"
              className="fp__button fp__button--primary"
              onClick={() => void createOne()}
              disabled={!canCreate || createBusy}
            >
              Add
            </button>
          </div>
        </div>

        {error && <div className="fp__error">{error}</div>}

        {!sessionUuid ? (
          <div className="fp__empty">
            No correlated invocation for this terminal yet.
          </div>
        ) : (
          <div className="fp__sections">
            <PromptSection
              title="Pending"
              empty="No queued follow-ups."
              entries={pending}
              editingId={editingId}
              editingText={editingText}
              busyId={busyId}
              canSend={session?.state === "live"}
              onEditText={setEditingText}
              onStartEdit={startEdit}
              onCancelEdit={cancelEdit}
              onSaveEdit={() => void saveEdit()}
              onSend={(entry) => void sendPrompt(entry)}
              onCopy={(entry) => void copyPrompt(entry)}
              onDelete={(entry) => void removePrompt(entry)}
            />
            <PromptSection
              title="Sent"
              empty="No sent follow-ups yet."
              entries={sent}
              editingId={editingId}
              editingText={editingText}
              busyId={busyId}
              canSend={false}
              onEditText={setEditingText}
              onStartEdit={startEdit}
              onCancelEdit={cancelEdit}
              onSaveEdit={() => void saveEdit()}
              onSend={() => {}}
              onCopy={(entry) => void copyPrompt(entry)}
              onDelete={(entry) => void removePrompt(entry)}
            />
          </div>
        )}
      </div>
    </Overlay>
  );
}

function PromptSection({
  title,
  empty,
  entries,
  editingId,
  editingText,
  busyId,
  canSend,
  onEditText,
  onStartEdit,
  onCancelEdit,
  onSaveEdit,
  onSend,
  onCopy,
  onDelete,
}: {
  title: string;
  empty: string;
  entries: FuturePromptEntry[];
  editingId: string | null;
  editingText: string;
  busyId: string | null;
  canSend: boolean;
  onEditText: (value: string) => void;
  onStartEdit: (entry: FuturePromptEntry) => void;
  onCancelEdit: () => void;
  onSaveEdit: () => void;
  onSend: (entry: FuturePromptEntry) => void;
  onCopy: (entry: FuturePromptEntry) => void;
  onDelete: (entry: FuturePromptEntry) => void;
}) {
  return (
    <section className="fp__section">
      <div className="fp__section-header">
        <span className="fp__section-title">{title}</span>
        <span className="fp__section-count">{entries.length}</span>
      </div>
      {entries.length === 0 ? (
        <div className="fp__section-empty">{empty}</div>
      ) : (
        <ul className="fp__list">
          {entries.map((entry) => {
            const editing = editingId === entry.id;
            const busy = busyId === entry.id;
            return (
              <li key={entry.id} className="fp__item">
                <div className="fp__item-meta">
                  <span className={`fp__state fp__state--${entry.state}`}>
                    {entry.state}
                  </span>
                  <span className="fp__timestamp">
                    {formatTimestamp(entry.updated_at ?? entry.created_at)}
                  </span>
                </div>
                {editing ? (
                  <textarea
                    className="fp__edit-input"
                    value={editingText}
                    onChange={(e) => onEditText(e.target.value)}
                    rows={4}
                    disabled={busy}
                  />
                ) : (
                  <pre className="fp__item-text">{entry.text}</pre>
                )}
                <div className="fp__item-actions">
                  {editing ? (
                    <>
                      <button
                        type="button"
                        className="fp__button fp__button--primary"
                        onClick={onSaveEdit}
                        disabled={busy || editingText.trim().length === 0}
                      >
                        Save
                      </button>
                      <button
                        type="button"
                        className="fp__button"
                        onClick={onCancelEdit}
                        disabled={busy}
                      >
                        Cancel
                      </button>
                    </>
                  ) : (
                    <>
                      {entry.state === "pending" && (
                        <button
                          type="button"
                          className="fp__button fp__button--primary"
                          onClick={() => onSend(entry)}
                          disabled={!canSend || busy}
                        >
                          Send
                        </button>
                      )}
                      <button
                        type="button"
                        className="fp__button"
                        onClick={() => onCopy(entry)}
                        disabled={busy}
                      >
                        Copy
                      </button>
                      <button
                        type="button"
                        className="fp__button"
                        onClick={() => onStartEdit(entry)}
                        disabled={busy}
                      >
                        Edit
                      </button>
                      <button
                        type="button"
                        className="fp__button fp__button--danger"
                        onClick={() => onDelete(entry)}
                        disabled={busy}
                      >
                        Delete
                      </button>
                    </>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

function sortPrompts(entries: FuturePromptEntry[]): FuturePromptEntry[] {
  const next = [...entries];
  next.sort((a, b) => {
    const rankA = a.state === "pending" ? 0 : 1;
    const rankB = b.state === "pending" ? 0 : 1;
    if (rankA !== rankB) return rankA - rankB;
    const timeA = Date.parse(a.updated_at ?? a.created_at ?? "") || 0;
    const timeB = Date.parse(b.updated_at ?? b.created_at ?? "") || 0;
    return a.state === "pending" ? timeA - timeB : timeB - timeA;
  });
  return next;
}

function formatTimestamp(raw: string | null): string {
  if (!raw) return "unknown time";
  const date = new Date(raw);
  if (Number.isNaN(date.getTime())) return raw;
  return date.toLocaleString();
}
