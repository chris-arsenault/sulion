import { useEffect } from "react";
import { create } from "zustand";

import {
  ApiError,
  createRepo as apiCreateRepo,
  createSession as apiCreateSession,
  deleteSession as apiDeleteSession,
  getAppState,
  updateSession as apiUpdateSession,
} from "../api/client";
import type {
  CreateRepoRequest,
  CreateSessionRequest,
  RepoView,
  SessionView,
  StatsResponse,
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

const POLL_APP_STATE_MS = 3_000;

export interface SessionStore {
  sessions: SessionView[];
  repos: RepoView[];
  stats: StatsResponse | null;
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
  loadAppState: () => Promise<void>;
}

function initialState(): Pick<
  SessionStore,
  | "sessions"
  | "repos"
  | "stats"
  | "selectedSessionId"
  | "lastError"
  | "sessionsLoaded"
  | "lastViewed"
> {
  return {
    sessions: [],
    repos: [],
    stats: null,
    selectedSessionId: readSessionIdFromUrl(),
    lastError: null,
    sessionsLoaded: false,
    lastViewed: loadLastViewedMap(),
  };
}

export const useSessionStore = create<SessionStore>()((set, get) => ({
  ...initialState(),

  async loadAppState() {
    try {
      const data = await getAppState();
      const sessions = Array.isArray(data.sessions) ? data.sessions : [];
      const repos = Array.isArray(data.repos) ? data.repos : [];
      const stats = data.stats ?? null;
      set((state) => {
        const sameSessions = sameFlatRecordArray(state.sessions, sessions);
        const sameRepos = sameJson(state.repos, repos);
        const sameStats = sameJson(state.stats, stats);
        if (
          sameSessions &&
          sameRepos &&
          sameStats &&
          state.sessionsLoaded &&
          state.lastError == null
        ) {
          return state;
        }
        return {
          sessions: sameSessions ? state.sessions : sessions,
          repos: sameRepos ? state.repos : repos,
          stats: sameStats ? state.stats : stats,
          lastError: null,
          sessionsLoaded: true,
        };
      });
    } catch (err) {
      console.error("getAppState failed", err);
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
    await get().loadAppState();
  },

  isUnread(sessionId, lastEventAt) {
    return isSessionUnread(get().lastViewed, sessionId, lastEventAt);
  },
}));

let consumerCount = 0;
let appStateTimer: ReturnType<typeof setInterval> | null = null;
let popstateAttached = false;

function syncSelectedSessionFromUrl() {
  useSessionStore.setState({ selectedSessionId: readSessionIdFromUrl() });
}

function startSessionStore() {
  if (typeof window === "undefined") return;
  consumerCount += 1;
  if (consumerCount > 1) return;

  void useSessionStore.getState().loadAppState();
  appStateTimer = window.setInterval(
    () => void useSessionStore.getState().loadAppState(),
    POLL_APP_STATE_MS,
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

  if (appStateTimer) {
    clearInterval(appStateTimer);
    appStateTimer = null;
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
  if (appStateTimer) {
    clearInterval(appStateTimer);
    appStateTimer = null;
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

function sameFlatRecordArray<T extends object>(
  current: T[],
  next: T[],
): boolean {
  if (current === next) return true;
  if (current.length !== next.length) return false;
  for (let i = 0; i < current.length; i += 1) {
    if (!sameFlatRecord(current[i]!, next[i]!)) return false;
  }
  return true;
}

function sameFlatRecord<T extends object>(a: T, b: T): boolean {
  if (a === b) return true;
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const aKeys = Object.keys(left);
  const bKeys = Object.keys(right);
  if (aKeys.length !== bKeys.length) return false;
  for (const key of aKeys) {
    if (left[key] !== right[key]) return false;
  }
  return true;
}

function sameJson(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}
