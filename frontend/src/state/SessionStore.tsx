// Central session/repo state. Exposes a single context consumed by the
// sidebar, terminal pane, and timeline pane. Fetches on mount and polls
// the session list every few seconds so multiple browser tabs stay in
// sync. Selection is URL-driven via ?session=<id>.
//
// Design choices documented in the #7 ticket:
//   - React context + useReducer rather than a store library; the
//     surface is small enough that another dep would be overhead.
//   - Sessions + repos polled independently; failures logged to console
//     but don't clear last-known-good state.

import {
  type ReactNode,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

import {
  ApiError,
  createRepo as apiCreateRepo,
  createSession as apiCreateSession,
  deleteSession as apiDeleteSession,
  updateSession as apiUpdateSession,
  listRepos,
  listSessions,
} from "../api/client";
import type {
  CreateRepoRequest,
  CreateSessionRequest,
  RepoView,
  SessionView,
  UpdateSessionRequest,
} from "../api/types";
import { useLastViewed } from "./useLastViewed";

const POLL_SESSIONS_MS = 3000;
const POLL_REPOS_MS = 10_000;

export interface SessionStore {
  sessions: SessionView[];
  repos: RepoView[];
  selectedSessionId: string | null;
  selectSession: (id: string | null) => void;
  createSession: (req: CreateSessionRequest) => Promise<SessionView>;
  deleteSession: (id: string) => Promise<void>;
  updateSession: (id: string, patch: UpdateSessionRequest) => Promise<void>;
  createRepo: (req: CreateRepoRequest) => Promise<RepoView>;
  refresh: () => Promise<void>;
  lastError: string | null;
  /** Unread indicator derived from server-reported last_event_at vs.
   * client-side last-viewed timestamps in localStorage (ticket #23). */
  isUnread: (sessionId: string, lastEventAt: string | null) => boolean;
}

const Ctx = createContext<SessionStore | null>(null);

export function SessionProvider({ children }: { children: ReactNode }) {
  const [sessions, setSessions] = useState<SessionView[]>([]);
  const [repos, setRepos] = useState<RepoView[]>([]);
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(() =>
    readSessionIdFromUrl(),
  );
  const [lastError, setLastError] = useState<string | null>(null);
  const lastViewed = useLastViewed();

  const loadSessions = useCallback(async () => {
    try {
      const data = await listSessions();
      setSessions(data.sessions);
    } catch (err) {
      console.error("listSessions failed", err);
      if (err instanceof ApiError) setLastError(err.message);
    }
  }, []);

  const loadRepos = useCallback(async () => {
    try {
      const data = await listRepos();
      setRepos(data.repos);
    } catch (err) {
      console.error("listRepos failed", err);
      if (err instanceof ApiError) setLastError(err.message);
    }
  }, []);

  useEffect(() => {
    void loadSessions();
    void loadRepos();
    const sTimer = setInterval(() => void loadSessions(), POLL_SESSIONS_MS);
    const rTimer = setInterval(() => void loadRepos(), POLL_REPOS_MS);
    return () => {
      clearInterval(sTimer);
      clearInterval(rTimer);
    };
  }, [loadSessions, loadRepos]);

  // Mirror URL ?session=<id> → state when the user navigates (back/forward).
  useEffect(() => {
    const onPopState = () => setSelectedSessionId(readSessionIdFromUrl());
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, []);

  const selectSession = useCallback(
    (id: string | null) => {
      setSelectedSessionId(id);
      writeSessionIdToUrl(id);
      if (id) lastViewed.markViewed(id);
    },
    [lastViewed],
  );

  const createSession = useCallback(
    async (req: CreateSessionRequest) => {
      const created = await apiCreateSession(req);
      setSessions((prev) => [created, ...prev]);
      selectSession(created.id);
      return created;
    },
    [selectSession],
  );

  const deleteSession = useCallback(
    async (id: string) => {
      await apiDeleteSession(id);
      setSessions((prev) => prev.filter((s) => s.id !== id));
      if (selectedSessionId === id) selectSession(null);
    },
    [selectedSessionId, selectSession],
  );

  const updateSession = useCallback(
    async (id: string, patch: UpdateSessionRequest) => {
      // Optimistic: apply locally first, roll back on error.
      let prev: SessionView[] = [];
      setSessions((current) => {
        prev = current;
        return current.map((s) =>
          s.id === id
            ? {
                ...s,
                ...(patch.label !== undefined ? { label: patch.label } : {}),
                ...(patch.pinned !== undefined ? { pinned: patch.pinned } : {}),
                ...(patch.color !== undefined ? { color: patch.color } : {}),
              }
            : s,
        );
      });
      try {
        await apiUpdateSession(id, patch);
      } catch (err) {
        setSessions(prev);
        throw err;
      }
    },
    [],
  );

  const createRepo = useCallback(async (req: CreateRepoRequest) => {
    const created = await apiCreateRepo(req);
    setRepos((prev) => [...prev, created].sort((a, b) => a.name.localeCompare(b.name)));
    return created;
  }, []);

  const refresh = useCallback(async () => {
    await Promise.all([loadSessions(), loadRepos()]);
  }, [loadSessions, loadRepos]);

  const value = useMemo<SessionStore>(
    () => ({
      sessions,
      repos,
      selectedSessionId,
      selectSession,
      createSession,
      deleteSession,
      updateSession,
      createRepo,
      refresh,
      lastError,
      isUnread: lastViewed.isUnread,
    }),
    [
      sessions,
      repos,
      selectedSessionId,
      selectSession,
      createSession,
      deleteSession,
      updateSession,
      createRepo,
      refresh,
      lastError,
      lastViewed.isUnread,
    ],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useSessions(): SessionStore {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useSessions called outside SessionProvider");
  return ctx;
}

function readSessionIdFromUrl(): string | null {
  if (typeof window === "undefined") return null;
  return new URLSearchParams(window.location.search).get("session");
}

function writeSessionIdToUrl(id: string | null) {
  if (typeof window === "undefined") return;
  const url = new URL(window.location.href);
  if (id) url.searchParams.set("session", id);
  else url.searchParams.delete("session");
  window.history.replaceState({}, "", url.toString());
}
