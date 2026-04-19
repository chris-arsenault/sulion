// Universal search. Three scopes: timeline (active session), repo
// (active repo), workspace (everything). Streams NDJSON hits from
// /api/search — fetch + parse run in a web worker, which batches hits
// at ~30Hz and posts them to the main thread. Keeps the UI responsive
// and the terminal WebSocket frames flowing during a big search.

import { useEffect, useMemo, useRef, useState } from "react";

import type { SearchHit, SearchScope } from "../api/types";
import { useSessions } from "../state/SessionStore";
import { useTabs } from "../state/TabStore";
import "./SearchTab.css";

interface Props {
  initialQuery?: string;
  initialScope?: SearchScope;
}

export function SearchTab({ initialQuery = "", initialScope = "workspace" }: Props) {
  const [query, setQuery] = useState(initialQuery);
  const [scope, setScope] = useState<SearchScope>(initialScope);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [running, setRunning] = useState(false);
  const [done, setDone] = useState(false);
  const workerRef = useRef<Worker | null>(null);

  const { sessions } = useSessions();
  const { openTab, tabs, activeByPane } = useTabs();
  // Derive "current context" from which tab is active in the top pane,
  // not from the sidebar highlight. Tabs are independent — the sidebar
  // is for navigation, not state.
  const focusedTabId = activeByPane.top ?? activeByPane.bottom ?? null;
  const focusedTab = focusedTabId ? tabs[focusedTabId] ?? null : null;
  const contextSessionId =
    focusedTab?.kind === "terminal" || focusedTab?.kind === "timeline"
      ? focusedTab.sessionId ?? null
      : null;
  const contextRepo = (() => {
    if (focusedTab?.repo) return focusedTab.repo;
    if (contextSessionId) {
      const s = sessions.find((x) => x.id === contextSessionId);
      return s?.repo ?? null;
    }
    return null;
  })();

  useEffect(() => {
    const w = new Worker(
      new URL("../workers/searchParser.worker.ts", import.meta.url),
      { type: "module" },
    );
    w.onmessage = (ev: MessageEvent<
      | { kind: "hits"; hits: SearchHit[] }
      | { kind: "done" }
      | { kind: "error"; message: string }
    >) => {
      const m = ev.data;
      if (m.kind === "hits") {
        setHits((prev) => [...prev, ...m.hits]);
      } else if (m.kind === "done") {
        setRunning(false);
        setDone(true);
      } else if (m.kind === "error") {
        setRunning(false);
        setDone(true);
      }
    };
    workerRef.current = w;
    return () => {
      w.postMessage({ kind: "abort" });
      w.terminate();
      workerRef.current = null;
    };
  }, []);

  const run = (q: string, s: SearchScope) => {
    workerRef.current?.postMessage({ kind: "abort" });
    if (!q.trim()) {
      setHits([]);
      setRunning(false);
      setDone(true);
      return;
    }
    const qs = new URLSearchParams();
    qs.set("q", q);
    qs.set("scope", s);
    if (s === "repo" && contextRepo) qs.set("repo", contextRepo);
    if (s === "timeline" && contextSessionId)
      qs.set("session", contextSessionId);
    setHits([]);
    setRunning(true);
    setDone(false);
    workerRef.current?.postMessage({
      kind: "start",
      url: `/api/search?${qs.toString()}`,
    });
  };

  const canUseTimelineScope = contextSessionId != null;
  const canUseRepoScope = contextRepo != null;

  const disabledNote = useMemo(() => {
    if (scope === "timeline" && !canUseTimelineScope) {
      return "Focus a terminal or timeline tab to search its timeline.";
    }
    if (scope === "repo" && !canUseRepoScope) {
      return "Focus a tab bound to a repo (terminal, timeline, file, or diff) to search that repo.";
    }
    return null;
  }, [scope, canUseTimelineScope, canUseRepoScope]);

  const onHitClick = (hit: SearchHit) => {
    if (hit.type === "file") {
      openTab({ kind: "file", repo: hit.repo, path: hit.path });
    } else if (hit.type === "event" && hit.session_id) {
      openTab({ kind: "timeline", sessionId: hit.session_id });
    }
  };

  return (
    <div className="st">
      <form
        className="st__form"
        onSubmit={(e) => {
          e.preventDefault();
          run(query, scope);
        }}
      >
        <input
          type="text"
          className="st__input"
          placeholder="search…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          autoFocus
        />
        <div className="st__scope-group" role="tablist" aria-label="Search scope">
          {(["timeline", "repo", "workspace"] as SearchScope[]).map((s) => (
            <button
              key={s}
              type="button"
              role="tab"
              aria-selected={scope === s}
              className={
                scope === s ? "st__scope st__scope--active" : "st__scope"
              }
              onClick={() => setScope(s)}
            >
              {s}
            </button>
          ))}
        </div>
        <button type="submit" className="st__run">
          {running ? "…" : "go"}
        </button>
      </form>
      {disabledNote && <div className="st__note">{disabledNote}</div>}
      <div className="st__results">
        {hits.length === 0 && done && !disabledNote && (
          <div className="st__muted">no matches</div>
        )}
        {hits.map((h, i) => (
          <HitRow key={i} hit={h} onClick={() => onHitClick(h)} />
        ))}
        {running && <div className="st__muted">streaming…</div>}
      </div>
    </div>
  );
}

function HitRow({ hit, onClick }: { hit: SearchHit; onClick: () => void }) {
  if (hit.type === "file") {
    return (
      <button type="button" className="st__hit st__hit--file" onClick={onClick}>
        <span className="st__hit-kind">file</span>
        <span className="st__hit-where">
          {hit.repo} · {hit.path}:{hit.line}
        </span>
        <span className="st__hit-preview">{hit.preview}</span>
      </button>
    );
  }
  if (hit.type === "event") {
    return (
      <button type="button" className="st__hit st__hit--event" onClick={onClick}>
        <span className="st__hit-kind">event</span>
        <span className="st__hit-where">
          {hit.kind} · {new Date(hit.timestamp).toLocaleTimeString()}
        </span>
        <span className="st__hit-preview">{hit.preview}</span>
      </button>
    );
  }
  return null;
}
