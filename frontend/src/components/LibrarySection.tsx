// Sidebar subsection: lists refs or prompts for one repo. Loads
// lazily on first mount and refreshes when a save/delete elsewhere
// emits a typed `library-changed` app command. Keeps the list small:
// each entry is a click-to-open link and a context menu for delete.
//
// Prompts sections get a "+" button that opens an inline form (name
// + body) — refs are created from the timeline (right-click a turn →
// "Pin as reference"), not from this section.

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
} from "react";

import {
  deleteLibraryEntry,
  listLibrary,
  saveLibraryEntry,
} from "../api/client";
import type { LibraryEntry, LibraryKind } from "../api/types";
import { appCommands, useAppCommand } from "../state/AppCommands";
import { useTabs } from "../state/TabStore";
import type { MenuItem } from "./common/ContextMenu";
import {
  contextMenuHandler,
  useContextMenu,
} from "./common/ContextMenu";

export function LibrarySection({
  repo,
  kind,
  open,
  onToggle,
}: {
  repo: string;
  kind: LibraryKind;
  open: boolean;
  onToggle: () => void;
}) {
  const [entries, setEntries] = useState<LibraryEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [creatingPrompt, setCreatingPrompt] = useState(false);
  const openTab = useTabs((store) => store.openTab);
  const openCtx = useContextMenu((store) => store.open);

  const refresh = useCallback(async () => {
    try {
      const list = await listLibrary(repo, kind);
      setEntries(list);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "load failed");
    }
  }, [repo, kind]);

  useEffect(() => {
    if (!open) return;
    if (entries === null) void refresh();
  }, [open, entries, refresh]);

  useAppCommand("library-changed", (command) => {
    if (command.repo !== repo) return;
    if (command.kind !== kind) return;
    void refresh();
  });

  const onEntryClick = (entry: LibraryEntry) => {
    openTab({
      kind: kind === "refs" ? "ref" : "prompt",
      repo,
      slug: entry.slug,
    });
  };

  const onEntryContextMenu = (entry: LibraryEntry) =>
    contextMenuHandler(openCtx, () => {
      const items: MenuItem[] = [
        {
          kind: "item",
          id: "open",
          label: "Open",
          onSelect: () => onEntryClick(entry),
        },
        { kind: "separator" },
        {
          kind: "item",
          id: "delete",
          label: "Delete",
          destructive: true,
          onSelect: async () => {
            if (!confirm(`Delete ${kind === "refs" ? "reference" : "prompt"} "${entry.name}"?`)) {
              return;
            }
            try {
              await deleteLibraryEntry(repo, kind, entry.slug);
              appCommands.libraryChanged({ repo, kind });
            } catch (err) {
              setError(err instanceof Error ? err.message : "delete failed");
            }
          },
        },
      ];
      return items;
    });

  const sectionLabel = kind === "refs" ? "References" : "Prompts";

  return (
    <div className="lib-sec">
      <div className="lib-sec__header">
        <button
          type="button"
          className="lib-sec__toggle"
          onClick={onToggle}
          aria-expanded={open}
        >
          <span
            className={
              open ? "lib-sec__chev lib-sec__chev--open" : "lib-sec__chev"
            }
          >
            ▸
          </span>
          <span className="lib-sec__label">{sectionLabel}</span>
          {entries && <span className="lib-sec__count">{entries.length}</span>}
        </button>
        {kind === "prompts" && open && (
          <button
            type="button"
            className="lib-sec__new"
            onClick={(e) => {
              e.stopPropagation();
              setCreatingPrompt((v) => !v);
            }}
            title="New prompt"
            aria-label="New prompt"
          >
            +
          </button>
        )}
      </div>
      {open && (
        <div className="lib-sec__body">
          {creatingPrompt && kind === "prompts" && (
            <NewPromptForm
              repo={repo}
              onCancel={() => setCreatingPrompt(false)}
              onCreated={() => {
                setCreatingPrompt(false);
                void refresh();
              }}
              onError={setError}
            />
          )}
          {error && <div className="lib-sec__error">{error}</div>}
          {entries === null && !error && (
            <div className="lib-sec__muted">loading…</div>
          )}
          {entries && entries.length === 0 && !error && !creatingPrompt && (
            <div className="lib-sec__muted">
              {kind === "refs"
                ? "right-click a timeline turn to pin it"
                : "no saved prompts"}
            </div>
          )}
          {entries && entries.length > 0 && (
            <ul className="lib-sec__list">
              {entries.map((e) => (
                <li key={e.slug}>
                  <button
                    type="button"
                    className="lib-sec__entry"
                    onClick={() => onEntryClick(e)}
                    onContextMenu={onEntryContextMenu(e)}
                    title={e.body.slice(0, 160)}
                  >
                    <span className="lib-sec__entry-name">{e.name}</span>
                    {e.tags.length > 0 && (
                      <span className="lib-sec__entry-tags">
                        {e.tags.slice(0, 3).map((t) => (
                          <span key={t} className="lib-sec__entry-tag">
                            {t}
                          </span>
                        ))}
                      </span>
                    )}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

function NewPromptForm({
  repo,
  onCancel,
  onCreated,
  onError,
}: {
  repo: string;
  onCancel: () => void;
  onCreated: () => void;
  onError: (msg: string) => void;
}) {
  const [name, setName] = useState("");
  const [body, setBody] = useState("");
  const [saving, setSaving] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    nameRef.current?.focus();
  }, []);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!name.trim() || !body.trim()) return;
    setSaving(true);
    try {
      await saveLibraryEntry(repo, "prompts", {
        name: name.trim(),
        tags: [],
        body,
      });
      appCommands.libraryChanged({ repo, kind: "prompts" });
      onCreated();
    } catch (err) {
      onError(err instanceof Error ? err.message : "save failed");
    } finally {
      setSaving(false);
    }
  };

  return (
    <form
      className="lib-sec__form"
      onSubmit={submit}
      onKeyDown={(e) => {
        if (e.key === "Escape") onCancel();
      }}
    >
      <input
        ref={nameRef}
        type="text"
        placeholder="prompt name"
        value={name}
        onChange={(e) => setName(e.target.value)}
        aria-label="Prompt name"
      />
      <textarea
        placeholder="prompt body"
        value={body}
        onChange={(e) => setBody(e.target.value)}
        rows={4}
        aria-label="Prompt body"
      />
      <div className="lib-sec__form-actions">
        <button type="button" onClick={onCancel} disabled={saving}>
          Cancel
        </button>
        <button type="submit" disabled={saving || !name.trim() || !body.trim()}>
          {saving ? "Saving…" : "Save prompt"}
        </button>
      </div>
    </form>
  );
}
