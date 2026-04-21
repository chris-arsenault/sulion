import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import { useShallow } from "zustand/react/shallow";

import {
  deleteLibraryEntry,
  listLibrary,
  saveLibraryEntry,
} from "../api/client";
import type { LibraryEntry, LibraryKind } from "../api/types";
import { appCommands, useAppCommand } from "../state/AppCommands";
import { useTabs } from "../state/TabStore";
import { Icon } from "../icons";
import { Tooltip } from "./ui";
import { ConfirmDialog } from "./common/ConfirmDialog";
import type { MenuItem } from "./common/contextMenuStore";
import {
  contextMenuHandler,
  useContextMenu,
} from "./common/contextMenuStore";

export function LibraryPanel() {
  const [references, setReferences] = useState<LibraryEntry[] | null>(null);
  const [prompts, setPrompts] = useState<LibraryEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [referencesOpen, setReferencesOpen] = useState(true);
  const [promptsOpen, setPromptsOpen] = useState(true);
  const [editingPrompt, setEditingPrompt] = useState<PromptDraft | null>(null);
  const [pendingDelete, setPendingDelete] = useState<{
    kind: LibraryKind;
    entry: LibraryEntry;
  } | null>(null);
  const openCtx = useContextMenu((store) => store.open);
  const { openTab, tabs, activeByPane } = useTabs(
    useShallow((store) => ({
      openTab: store.openTab,
      tabs: store.tabs,
      activeByPane: store.activeByPane,
    })),
  );

  const refresh = useCallback(async (kind: LibraryKind) => {
    try {
      const entries = await listLibrary(kind);
      if (kind === "references") {
        setReferences(entries);
      } else {
        setPrompts(entries);
      }
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "load failed");
    }
  }, []);

  useEffect(() => {
    void refresh("references");
    void refresh("prompts");
  }, [refresh]);

  useAppCommand("library-changed", (command) => {
    void refresh(command.kind);
  });

  const activeTerminalSessionId = (() => {
    const topId = activeByPane.top;
    const bottomId = activeByPane.bottom;
    const candidate =
      (topId && tabs[topId]?.kind === "terminal" ? tabs[topId] : null) ??
      (bottomId && tabs[bottomId]?.kind === "terminal" ? tabs[bottomId] : null);
    return candidate?.sessionId ?? null;
  })();

  const injectPrompt = (entry: LibraryEntry) => {
    if (!activeTerminalSessionId) {
      setError("No active terminal tab to inject into.");
      return;
    }
    appCommands.injectTerminal({ sessionId: activeTerminalSessionId, text: entry.body });
    setError(null);
  };

  const copyBody = async (entry: LibraryEntry) => {
    try {
      await navigator.clipboard.writeText(entry.body);
    } catch {
      // HTTP deployments may not have clipboard permission.
    }
  };

  const requestDelete = (kind: LibraryKind, entry: LibraryEntry) => {
    setPendingDelete({ kind, entry });
  };

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    const { kind, entry } = pendingDelete;
    setPendingDelete(null);
    try {
      await deleteLibraryEntry(kind, entry.slug);
      appCommands.libraryChanged({ kind });
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "delete failed");
    }
  };

  const cancelDelete = useCallback(() => setPendingDelete(null), []);

  const onReferenceContextMenu = (entry: LibraryEntry) =>
    contextMenuHandler(openCtx, () => {
      const items: MenuItem[] = [
        {
          kind: "item",
          id: "open",
          label: "Open",
          onSelect: () => openTab({ kind: "ref", slug: entry.slug }),
        },
        {
          kind: "item",
          id: "copy",
          label: "Copy body",
          onSelect: () => void copyBody(entry),
        },
        { kind: "separator" },
        {
          kind: "item",
          id: "delete",
          label: "Delete",
          destructive: true,
          onSelect: () => requestDelete("references", entry),
        },
      ];
      return items;
    });

  const onPromptContextMenu = (entry: LibraryEntry) =>
    contextMenuHandler(openCtx, () => {
      const items: MenuItem[] = [
        {
          kind: "item",
          id: "inject",
          label: "Inject into terminal",
          onSelect: () => injectPrompt(entry),
        },
        {
          kind: "item",
          id: "copy",
          label: "Copy body",
          onSelect: () => void copyBody(entry),
        },
        {
          kind: "item",
          id: "edit",
          label: "Edit",
          onSelect: () =>
            setEditingPrompt({ slug: entry.slug, name: entry.name, body: entry.body }),
        },
        { kind: "separator" },
        {
          kind: "item",
          id: "delete",
          label: "Delete",
          destructive: true,
          onSelect: () => requestDelete("prompts", entry),
        },
      ];
      return items;
    });

  const savePrompt = useCallback(async (draft: PromptDraft) => {
    try {
      await saveLibraryEntry(
        "prompts",
        { name: draft.name.trim(), body: draft.body },
        draft.slug,
      );
      appCommands.libraryChanged({ kind: "prompts" });
      setEditingPrompt(null);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "save failed");
    }
  }, []);

  const toggleReferences = useCallback(
    () => setReferencesOpen((v) => !v),
    [],
  );
  const togglePrompts = useCallback(() => setPromptsOpen((v) => !v), []);
  const startNewPrompt = useCallback(
    () => setEditingPrompt({ slug: undefined, name: "", body: "" }),
    [],
  );
  const cancelEdit = useCallback(() => setEditingPrompt(null), []);

  return (
    <div className="lib-panel">
      <div className="lib-panel__header">
        <span className="sidebar__sub-label">Library</span>
      </div>
      {error && <div className="lib-sec__error">{error}</div>}

      <LibrarySection
        label="References"
        count={references?.length}
        open={referencesOpen}
        onToggle={toggleReferences}
      >
        {references === null && <div className="lib-sec__muted">loading…</div>}
        {references?.length === 0 && (
          <div className="lib-sec__muted">
            save assistant output from the timeline to keep it close by
          </div>
        )}
        {references && references.length > 0 && (
          <ul className="lib-sec__list">
            {references.map((entry) => (
              <ReferenceRow
                key={entry.slug}
                entry={entry}
                openTab={openTab}
                onContextMenu={onReferenceContextMenu(entry)}
              />
            ))}
          </ul>
        )}
      </LibrarySection>

      <LibrarySection
        label="Prompts"
        count={prompts?.length}
        open={promptsOpen}
        onToggle={togglePrompts}
        rightSlot={
          <Tooltip label="New prompt">
            <button
              type="button"
              className="lib-sec__new"
              onClick={startNewPrompt}
              aria-label="New prompt"
            >
              <Icon name="plus" size={12} />
            </button>
          </Tooltip>
        }
      >
        {editingPrompt && (
          <PromptForm
            draft={editingPrompt}
            onCancel={cancelEdit}
            onSave={savePrompt}
          />
        )}
        {prompts === null && <div className="lib-sec__muted">loading…</div>}
        {prompts?.length === 0 && !editingPrompt && (
          <div className="lib-sec__muted">
            save a past user prompt or create one here for one-click reuse
          </div>
        )}
        {prompts && prompts.length > 0 && (
          <ul className="lib-sec__list">
            {prompts.map((entry) => (
              <PromptRow
                key={entry.slug}
                entry={entry}
                injectPrompt={injectPrompt}
                onContextMenu={onPromptContextMenu(entry)}
              />
            ))}
          </ul>
        )}
      </LibrarySection>

      {pendingDelete && (
        <ConfirmDialog
          title={
            pendingDelete.kind === "references"
              ? "Delete reference?"
              : "Delete prompt?"
          }
          message={`"${pendingDelete.entry.name}" will be removed from the library. This can't be undone.`}
          confirmLabel="Delete"
          destructive
          onConfirm={() => void confirmDelete()}
          onCancel={cancelDelete}
        />
      )}
    </div>
  );
}

function LibrarySection({
  label,
  count,
  open,
  onToggle,
  rightSlot,
  children,
}: {
  label: string;
  count?: number;
  open: boolean;
  onToggle: () => void;
  rightSlot?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="lib-sec">
      <div className="lib-sec__header">
        <button
          type="button"
          className="lib-sec__toggle"
          onClick={onToggle}
          aria-expanded={open}
        >
          <span className={open ? "lib-sec__chev lib-sec__chev--open" : "lib-sec__chev"}>
            ▸
          </span>
          <span className="lib-sec__label">{label}</span>
          {count != null && <span className="lib-sec__count">{count}</span>}
        </button>
        {rightSlot}
      </div>
      {open && <div className="lib-sec__body">{children}</div>}
    </div>
  );
}

interface PromptDraft {
  slug?: string;
  name: string;
  body: string;
}

function PromptForm({
  draft,
  onCancel,
  onSave,
}: {
  draft: PromptDraft;
  onCancel: () => void;
  onSave: (draft: PromptDraft) => Promise<void> | void;
}) {
  const [name, setName] = useState(draft.name);
  const [body, setBody] = useState(draft.body);
  const [saving, setSaving] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setName(draft.name);
    setBody(draft.body);
    nameRef.current?.focus();
  }, [draft]);

  const submit = useCallback(
    async (event: FormEvent) => {
      event.preventDefault();
      if (!name.trim() || !body.trim()) return;
      setSaving(true);
      try {
        await onSave({ slug: draft.slug, name, body });
      } finally {
        setSaving(false);
      }
    },
    [name, body, onSave, draft.slug],
  );
  const onFormSubmit = useCallback(
    (event: FormEvent) => void submit(event),
    [submit],
  );

  const cancelOnEscape = useCallback(
    (event: React.KeyboardEvent<HTMLInputElement | HTMLTextAreaElement>) => {
      if (event.key === "Escape") onCancel();
    },
    [onCancel],
  );
  const onNameChange = useCallback(
    (event: React.ChangeEvent<HTMLInputElement>) => setName(event.target.value),
    [],
  );
  const onBodyChange = useCallback(
    (event: React.ChangeEvent<HTMLTextAreaElement>) =>
      setBody(event.target.value),
    [],
  );
  return (
    <form className="lib-sec__form" onSubmit={onFormSubmit}>
      <input
        ref={nameRef}
        type="text"
        placeholder="prompt name"
        value={name}
        onChange={onNameChange}
        onKeyDown={cancelOnEscape}
        aria-label="Prompt name"
      />
      <textarea
        placeholder="prompt body"
        value={body}
        onChange={onBodyChange}
        onKeyDown={cancelOnEscape}
        rows={4}
        aria-label="Prompt body"
      />
      <div className="lib-sec__form-actions">
        <button type="button" onClick={onCancel} disabled={saving}>
          Cancel
        </button>
        <button type="submit" disabled={saving || !name.trim() || !body.trim()}>
          {saving ? "Saving…" : draft.slug ? "Save changes" : "Save prompt"}
        </button>
      </div>
    </form>
  );
}

function ReferenceRow({
  entry,
  openTab,
  onContextMenu,
}: {
  entry: LibraryEntry;
  openTab: (tab: { kind: "ref"; slug: string }) => void;
  onContextMenu: (e: React.MouseEvent) => void;
}) {
  const onClick = useCallback(
    () => openTab({ kind: "ref", slug: entry.slug }),
    [openTab, entry.slug],
  );
  return (
    <li>
      <Tooltip label={entry.body.slice(0, 200)}>
        <button
          type="button"
          className="lib-sec__entry"
          onClick={onClick}
          onContextMenu={onContextMenu}
        >
          <span className="lib-sec__entry-name">{entry.name}</span>
          <span className="lib-sec__entry-preview">{preview(entry.body)}</span>
        </button>
      </Tooltip>
    </li>
  );
}

function PromptRow({
  entry,
  injectPrompt,
  onContextMenu,
}: {
  entry: LibraryEntry;
  injectPrompt: (entry: LibraryEntry) => void;
  onContextMenu: (e: React.MouseEvent) => void;
}) {
  const onClick = useCallback(
    () => injectPrompt(entry),
    [injectPrompt, entry],
  );
  return (
    <li>
      <Tooltip label={entry.body.slice(0, 200)}>
        <button
          type="button"
          className="lib-sec__entry"
          onClick={onClick}
          onContextMenu={onContextMenu}
        >
          <span className="lib-sec__entry-name">{entry.name}</span>
          <span className="lib-sec__entry-preview">{preview(entry.body)}</span>
        </button>
      </Tooltip>
    </li>
  );
}

function preview(text: string): string {
  return text.replace(/\s+/g, " ").trim().slice(0, 96) || "empty";
}
