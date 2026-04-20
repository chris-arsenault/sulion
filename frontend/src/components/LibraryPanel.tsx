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
import type { MenuItem } from "./common/ContextMenu";
import {
  contextMenuHandler,
  useContextMenu,
} from "./common/ContextMenu";

export function LibraryPanel() {
  const [references, setReferences] = useState<LibraryEntry[] | null>(null);
  const [prompts, setPrompts] = useState<LibraryEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [referencesOpen, setReferencesOpen] = useState(true);
  const [promptsOpen, setPromptsOpen] = useState(true);
  const [editingPrompt, setEditingPrompt] = useState<PromptDraft | null>(null);
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

  const deleteEntry = async (kind: LibraryKind, entry: LibraryEntry) => {
    if (!confirm(`Delete ${kind === "references" ? "reference" : "prompt"} "${entry.name}"?`)) {
      return;
    }
    try {
      await deleteLibraryEntry(kind, entry.slug);
      appCommands.libraryChanged({ kind });
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "delete failed");
    }
  };

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
          onSelect: () => void deleteEntry("references", entry),
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
          onSelect: () => void deleteEntry("prompts", entry),
        },
      ];
      return items;
    });

  const savePrompt = async (draft: PromptDraft) => {
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
  };

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
        onToggle={() => setReferencesOpen((value) => !value)}
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
              <li key={entry.slug}>
                <Tooltip label={entry.body.slice(0, 200)}>
                  <button
                    type="button"
                    className="lib-sec__entry"
                    onClick={() => openTab({ kind: "ref", slug: entry.slug })}
                    onContextMenu={onReferenceContextMenu(entry)}
                  >
                    <span className="lib-sec__entry-name">{entry.name}</span>
                    <span className="lib-sec__entry-preview">{preview(entry.body)}</span>
                  </button>
                </Tooltip>
              </li>
            ))}
          </ul>
        )}
      </LibrarySection>

      <LibrarySection
        label="Prompts"
        count={prompts?.length}
        open={promptsOpen}
        onToggle={() => setPromptsOpen((value) => !value)}
        rightSlot={
          <Tooltip label="New prompt">
            <button
              type="button"
              className="lib-sec__new"
              onClick={() => setEditingPrompt({ slug: undefined, name: "", body: "" })}
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
            onCancel={() => setEditingPrompt(null)}
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
              <li key={entry.slug}>
                <Tooltip label={entry.body.slice(0, 200)}>
                  <button
                    type="button"
                    className="lib-sec__entry"
                    onClick={() => injectPrompt(entry)}
                    onContextMenu={onPromptContextMenu(entry)}
                  >
                    <span className="lib-sec__entry-name">{entry.name}</span>
                    <span className="lib-sec__entry-preview">{preview(entry.body)}</span>
                  </button>
                </Tooltip>
              </li>
            ))}
          </ul>
        )}
      </LibrarySection>
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

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!name.trim() || !body.trim()) return;
    setSaving(true);
    try {
      await onSave({ slug: draft.slug, name, body });
    } finally {
      setSaving(false);
    }
  };

  return (
    <form
      className="lib-sec__form"
      onSubmit={(event) => void submit(event)}
      onKeyDown={(event) => {
        if (event.key === "Escape") onCancel();
      }}
    >
      <input
        ref={nameRef}
        type="text"
        placeholder="prompt name"
        value={name}
        onChange={(event) => setName(event.target.value)}
        aria-label="Prompt name"
      />
      <textarea
        placeholder="prompt body"
        value={body}
        onChange={(event) => setBody(event.target.value)}
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

function preview(text: string): string {
  return text.replace(/\s+/g, " ").trim().slice(0, 96) || "empty";
}
