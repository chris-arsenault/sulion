// Universal search. Three scopes: timeline / repo / workspace. Owns
// its own query + scope state locally — the tab registry only knows
// "this is a search tab". No subscription to other tabs or the
// sidebar. User explicitly selects the scope.
//
// Streams NDJSON hits from /api/search via a web worker that parses
// the stream off the main thread and batches results at ~30Hz so
// large searches don't thrash React or starve the terminal WS.

import { useEffect, useRef, useState } from "react";

import type { SearchHit, SearchScope } from "../api/types";
import { useTabs } from "../state/TabStore";
import "./SearchTab.css";

export function SearchTab() {
  const [query, setQuery] = useState("");
  const [scope, setScope] = useState<SearchScope>("workspace");
  // Repo/session pickers for the scoped modes. Also SearchTab-local —
  // no reaching up into the registry to infer "what is the user
  // looking at right now".
  const [repo, setRepo] = useState<string>("");
  const [session, setSession] = useState<string>("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [running, setRunning] = useState(false);
  const [done, setDone] = useState(false);
  const workerRef = useRef<Worker | null>(null);

  const openTab = useTabs((store) => store.openTab);

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
      } else if (m.kind === "done" || m.kind === "error") {
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
    if (s === "repo" && repo) qs.set("repo", repo);
    if (s === "timeline" && session) qs.set("session", session);
    setHits([]);
    setRunning(true);
    setDone(false);
    workerRef.current?.postMessage({
      kind: "start",
      url: `/api/search?${qs.toString()}`,
    });
  };

  const onHitClick = (hit: SearchHit) => {
    if (hit.type === "file") {
      openTab({ kind: "file", repo: hit.repo, path: hit.path });
    } else if (hit.type === "event" && hit.session_id) {
      openTab({ kind: "timeline", sessionId: hit.session_id });
    }
  };

  const needsRepo = scope === "repo" && !repo;
  const needsSession = scope === "timeline" && !session;

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
              className={scope === s ? "st__scope st__scope--active" : "st__scope"}
              onClick={() => setScope(s)}
            >
              {s}
            </button>
          ))}
        </div>
        {scope === "repo" && (
          <input
            type="text"
            className="st__input st__input--narrow"
            placeholder="repo"
            value={repo}
            onChange={(e) => setRepo(e.target.value)}
            aria-label="Repo to search"
          />
        )}
        {scope === "timeline" && (
          <input
            type="text"
            className="st__input st__input--narrow"
            placeholder="session id"
            value={session}
            onChange={(e) => setSession(e.target.value)}
            aria-label="Session id to search"
          />
        )}
        <button type="submit" className="st__run" disabled={needsRepo || needsSession}>
          {running ? "…" : "go"}
        </button>
      </form>
      {needsRepo && (
        <div className="st__note">Enter a repo name to search its files.</div>
      )}
      {needsSession && (
        <div className="st__note">Enter a session id to search its timeline.</div>
      )}
      <div className="st__results">
        {hits.length === 0 && done && !needsRepo && !needsSession && (
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
