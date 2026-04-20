// Reference preview tab. Renders a saved global library reference.

import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { deleteLibraryEntry, getLibraryEntry } from "../api/client";
import type { LibraryEntry } from "../api/types";
import { appCommands } from "../state/AppCommands";
import { useTabs } from "../state/TabStore";
import {
  contextMenuHandler,
  useContextMenu,
} from "./common/ContextMenu";
import { copyToClipboard } from "./terminal/clipboard";
import { Markdown } from "./timeline/Markdown";
import { Tooltip } from "./ui";
import "./LibraryTab.css";

export function RefTab({ slug }: { slug: string }) {
  const [entry, setEntry] = useState<LibraryEntry | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { closeTab, tabs } = useTabs(
    useShallow((store) => ({
      closeTab: store.closeTab,
      tabs: store.tabs,
    })),
  );
  const openCtx = useContextMenu((store) => store.open);

  useEffect(() => {
    let cancelled = false;
    setEntry(null);
    setError(null);
    getLibraryEntry("references", slug)
      .then((e) => {
        if (!cancelled) setEntry(e);
      })
      .catch((err) =>
        setError(err instanceof Error ? err.message : "load failed"),
      );
    return () => {
      cancelled = true;
    };
  }, [slug]);

  const onDelete = async () => {
    if (!confirm(`Delete reference "${entry?.name ?? slug}"?`)) return;
    try {
      await deleteLibraryEntry("references", slug);
      appCommands.libraryChanged({ kind: "references" });
      // Close our own tab (find the id by matching).
      const mine = Object.values(tabs).find(
        (t) => t.kind === "ref" && t.slug === slug,
      );
      if (mine) closeTab(mine.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : "delete failed");
    }
  };

  const onCopy = async () => {
    if (!entry) return;
    await copyToClipboard(entry.body);
  };

  if (error) {
    return (
      <div className="lib-tab lib-tab--err">
        <div className="lib-tab__header">
          <span className="lib-tab__path">
            reference · {slug}
          </span>
        </div>
        <div className="lib-tab__body">error: {error}</div>
      </div>
    );
  }
  if (!entry) {
    return (
      <div className="lib-tab">
        <div className="lib-tab__header">
          <span className="lib-tab__path">
            reference · {slug}
          </span>
        </div>
        <div className="lib-tab__body lib-tab__body--muted">loading…</div>
      </div>
    );
  }

  const onContextMenu = contextMenuHandler(openCtx, () => [
    {
      kind: "item",
      id: "copy-body",
      label: "Copy body",
      onSelect: () => void onCopy(),
    },
    { kind: "separator" },
    {
      kind: "item",
      id: "delete-reference",
      label: "Delete reference",
      destructive: true,
      onSelect: () => void onDelete(),
    },
  ]);

  return (
    <div className="lib-tab" onContextMenu={onContextMenu}>
      <Tooltip label="Right-click for reference actions">
        <div className="lib-tab__header">
          <span className="lib-tab__path">
          <strong className="lib-tab__title">{entry.name}</strong>
          <span className="lib-tab__muted">· reference · {entry.slug}</span>
        </span>
          <span className="lib-tab__meta">
            {entry.updated_at && <span>updated {formatDate(entry.updated_at)}</span>}
          </span>
        </div>
      </Tooltip>
      <div className="lib-tab__body">
        <Markdown source={entry.body} />
      </div>
    </div>
  );
}

function formatDate(iso: string): string {
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}
