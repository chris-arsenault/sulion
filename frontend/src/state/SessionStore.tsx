import { useEffect } from "react";
import { create } from "zustand";

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
import {
  clearLastViewedStorage,
  isSessionUnread,
  loadLastViewedMap,
  markLastViewed,
  saveLastViewedMap,
  type LastViewedMap,
} from "./useLastViewed";

const POLL_SESSIONS_MS = 3_000;
const POLL_REPOS_MS = 10_000;

export interface SessionStore {
  sessions: SessionView[];
  repos: RepoView[];
  selectedSessionId: string | null;
  lastError: string | null;
  sessionsLoaded: boolean;
  lastViewed: LastViewedMap;
  selectSession: (id: string | null) => void;
  createSession: (req: CreateSessionRequest) => Promise<SessionView>;
  deleteSession: (id: string) => Promise<void>;
  updateSession: (id: string, patch: UpdateSessionRequest) => Promise<void>;
  createRepo: (req: CreateRepoRequest) => Promise<RepoView>;
  refresh: () => Promise<void>;
  isUnread: (sessionId: string, lastEventAt: string | null) => boolean;
  loadSessions: () => Promise<void>;
  loadRepos: () => Promise<void>;
}

function initialState(): Pick<
  SessionStore,
  "sessions" | "repos" | "selectedSessionId" | "lastError" | "sessionsLoaded" | "lastViewed"
> {
  return {
    sessions: [],
    repos: [],
    selectedSessionId: readSessionIdFromUrl(),
    lastError: null,
    sessionsLoaded: false,
    lastViewed: loadLastViewedMap(),
  };
}

export const useSessionStore = create<SessionStore>()((set, get) => ({
  ...initialState(),

  async loadSessions() {
    try {
      const data = await listSessions();
      set({ sessions: data.sessions, sessionsLoaded: true });
    } catch (err) {
      console.error("listSessions failed", err);
      if (err instanceof ApiError) set({ lastError: err.message });
    }
  },

  async loadRepos() {
    try {
      const data = await listRepos();
      set({ repos: data.repos });
    } catch (err) {
      console.error("listRepos failed", err);
      if (err instanceof ApiError) set({ lastError: err.message });
    }
  },

  selectSession(id) {
    set((state) => {
      if (!id) return { selectedSessionId: null };
      const nextViewed = markLastViewed(state.lastViewed, id);
      saveLastViewedMap(nextViewed);
      return { selectedSessionId: id, lastViewed: nextViewed };
    });
    writeSessionIdToUrl(id);
  },

  async createSession(req) {
    const created = await apiCreateSession(req);
    set((state) => {
      const nextViewed = markLastViewed(state.lastViewed, created.id);
      saveLastViewedMap(nextViewed);
      return {
        sessions: [created, ...state.sessions],
        selectedSessionId: created.id,
        lastViewed: nextViewed,
      };
    });
    writeSessionIdToUrl(created.id);
    return created;
  },

  async deleteSession(id) {
    await apiDeleteSession(id);
    const { selectedSessionId } = get();
    set((state) => ({
      sessions: state.sessions.filter((session) => session.id !== id),
      selectedSessionId: selectedSessionId === id ? null : state.selectedSessionId,
    }));
    if (selectedSessionId === id) writeSessionIdToUrl(null);
  },

  async updateSession(id, patch) {
    const prevSessions = get().sessions;
    set((state) => ({
      sessions: state.sessions.map((session) =>
        session.id === id
          ? {
              ...session,
              ...(patch.label !== undefined ? { label: patch.label } : {}),
              ...(patch.pinned !== undefined ? { pinned: patch.pinned } : {}),
              ...(patch.color !== undefined ? { color: patch.color } : {}),
            }
          : session,
      ),
    }));
    try {
      await apiUpdateSession(id, patch);
    } catch (err) {
      set({ sessions: prevSessions });
      throw err;
    }
  },

  async createRepo(req) {
    const created = await apiCreateRepo(req);
    set((state) => ({
      repos: [...state.repos, created].sort((a, b) => a.name.localeCompare(b.name)),
    }));
    return created;
  },

  async refresh() {
    await Promise.all([get().loadSessions(), get().loadRepos()]);
  },

  isUnread(sessionId, lastEventAt) {
    return isSessionUnread(get().lastViewed, sessionId, lastEventAt);
  },
}));

let consumerCount = 0;
let sessionsTimer: ReturnType<typeof setInterval> | null = null;
let reposTimer: ReturnType<typeof setInterval> | null = null;
let popstateAttached = false;

function syncSelectedSessionFromUrl() {
  useSessionStore.setState({ selectedSessionId: readSessionIdFromUrl() });
}

function startSessionStore() {
  if (typeof window === "undefined") return;
  consumerCount += 1;
  if (consumerCount > 1) return;

  void useSessionStore.getState().loadSessions();
  void useSessionStore.getState().loadRepos();
  sessionsTimer = window.setInterval(
    () => void useSessionStore.getState().loadSessions(),
    POLL_SESSIONS_MS,
  );
  reposTimer = window.setInterval(
    () => void useSessionStore.getState().loadRepos(),
    POLL_REPOS_MS,
  );
  if (!popstateAttached) {
    window.addEventListener("popstate", syncSelectedSessionFromUrl);
    popstateAttached = true;
  }
}

function stopSessionStore() {
  if (typeof window === "undefined") return;
  consumerCount = Math.max(0, consumerCount - 1);
  if (consumerCount > 0) return;

  if (sessionsTimer) {
    clearInterval(sessionsTimer);
    sessionsTimer = null;
  }
  if (reposTimer) {
    clearInterval(reposTimer);
    reposTimer = null;
  }
  if (popstateAttached) {
    window.removeEventListener("popstate", syncSelectedSessionFromUrl);
    popstateAttached = false;
  }
}

export function useSessions<T>(selector: (state: SessionStore) => T): T {
  useEffect(() => {
    startSessionStore();
    return stopSessionStore;
  }, []);
  return useSessionStore(selector);
}

export function resetSessionStore() {
  consumerCount = 0;
  if (sessionsTimer) {
    clearInterval(sessionsTimer);
    sessionsTimer = null;
  }
  if (reposTimer) {
    clearInterval(reposTimer);
    reposTimer = null;
  }
  if (typeof window !== "undefined" && popstateAttached) {
    window.removeEventListener("popstate", syncSelectedSessionFromUrl);
  }
  popstateAttached = false;
  useSessionStore.setState(initialState());
}

export function resetSessionStoreStorage() {
  clearLastViewedStorage();
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
