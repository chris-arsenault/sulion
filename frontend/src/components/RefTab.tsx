// Reference preview tab. Renders a saved markdown reference from
// `.shuttlecraft/refs/<slug>.md` in its repo.

import { useEffect, useState } from "react";

import { deleteLibraryEntry, getLibraryEntry } from "../api/client";
import type { LibraryEntry } from "../api/types";
import { useTabs } from "../state/TabStore";
import { Markdown } from "./timeline/Markdown";
import "./LibraryTab.css";

export function RefTab({ repo, slug }: { repo: string; slug: string }) {
  const [entry, setEntry] = useState<LibraryEntry | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { closeTab, tabs } = useTabs();

  useEffect(() => {
    let cancelled = false;
    setEntry(null);
    setError(null);
    getLibraryEntry(repo, "refs", slug)
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
    if (!confirm(`Delete reference "${entry?.name ?? slug}"?`)) return;
    try {
      await deleteLibraryEntry(repo, "refs", slug);
      // Close our own tab (find the id by matching).
      const mine = Object.values(tabs).find(
        (t) => t.kind === "ref" && t.repo === repo && t.slug === slug,
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
      /* HTTP deploys don't have clipboard perm; silent */
    }
  };

  if (error) {
    return (
      <div className="lib-tab lib-tab--err">
        <div className="lib-tab__header">
          <span className="lib-tab__path">
            refs · {repo} · {slug}
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
            refs · {repo} · {slug}
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
            · refs · {repo} · {entry.slug}
          </span>
        </span>
        <span className="lib-tab__meta">
          {entry.created_at && (
            <span>saved {formatDate(entry.created_at)}</span>
          )}
        </span>
        <button type="button" className="lib-tab__btn" onClick={onCopy}>
          copy body
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
