// Prompt preview tab. Same shape as RefTab but adds an "inject into
// terminal" action that dispatches a window event the TerminalPane
// listens for and pipes the body into the PTY via term.paste().

import { useEffect, useState } from "react";

import { deleteLibraryEntry, getLibraryEntry } from "../api/client";
import type { LibraryEntry } from "../api/types";
import { useTabs } from "../state/TabStore";
import "./LibraryTab.css";

export function PromptTab({ repo, slug }: { repo: string; slug: string }) {
  const [entry, setEntry] = useState<LibraryEntry | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { closeTab, tabs, activeByPane } = useTabs();

  useEffect(() => {
    let cancelled = false;
    setEntry(null);
    setError(null);
    getLibraryEntry(repo, "prompts", slug)
      .then((e) => {
        if (!cancelled) setEntry(e);
      })
      .catch((err) =>
        setError(err instanceof Error ? err.message : "load failed"),
      );
    return () => {
      cancelled = true;
    };
  }, [repo, slug]);

  const onDelete = async () => {
    if (!confirm(`Delete prompt "${entry?.name ?? slug}"?`)) return;
    try {
      await deleteLibraryEntry(repo, "prompts", slug);
      const mine = Object.values(tabs).find(
        (t) => t.kind === "prompt" && t.repo === repo && t.slug === slug,
      );
      if (mine) closeTab(mine.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : "delete failed");
    }
  };

  const onCopy = async () => {
    if (!entry) return;
    try {
      await navigator.clipboard.writeText(entry.body);
    } catch {
      /* silent */
    }
  };

  const onInject = () => {
    if (!entry) return;
    // Find the active terminal tab to target. Prefer top pane.
    const topId = activeByPane.top;
    const botId = activeByPane.bottom;
    const candidate =
      (topId && tabs[topId]?.kind === "terminal" ? tabs[topId] : null) ??
      (botId && tabs[botId]?.kind === "terminal" ? tabs[botId] : null);
    if (!candidate?.sessionId) {
      setError("No active terminal tab to inject into.");
      return;
    }
    window.dispatchEvent(
      new CustomEvent("shuttlecraft:inject-terminal", {
        detail: { sessionId: candidate.sessionId, text: entry.body },
      }),
    );
  };

  if (error) {
    return (
      <div className="lib-tab lib-tab--err">
        <div className="lib-tab__header">
          <span className="lib-tab__path">
            prompts · {repo} · {slug}
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
            prompts · {repo} · {slug}
          </span>
        </div>
        <div className="lib-tab__body lib-tab__body--muted">loading…</div>
      </div>
    );
  }

  return (
    <div className="lib-tab">
      <div className="lib-tab__header">
        <span className="lib-tab__path">
          <strong className="lib-tab__title">{entry.name}</strong>
          <span className="lib-tab__muted">
            · prompts · {repo} · {entry.slug}
          </span>
        </span>
        <span className="lib-tab__meta">
          {entry.created_at && (
            <span>saved {formatDate(entry.created_at)}</span>
          )}
        </span>
        <button
          type="button"
          className="lib-tab__btn lib-tab__btn--primary"
          onClick={onInject}
        >
          inject into terminal
        </button>
        <button type="button" className="lib-tab__btn" onClick={onCopy}>
          copy
        </button>
        <button
          type="button"
          className="lib-tab__btn lib-tab__btn--destructive"
          onClick={onDelete}
        >
          delete
        </button>
      </div>
      {entry.tags.length > 0 && (
        <div className="lib-tab__tags">
          {entry.tags.map((t) => (
            <span key={t} className="lib-tab__tag">
              {t}
            </span>
          ))}
        </div>
      )}
      <pre className="lib-tab__body lib-tab__body--pre">{entry.body}</pre>
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
