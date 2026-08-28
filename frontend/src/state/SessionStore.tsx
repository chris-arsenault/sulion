import { useEffect } from "react";
import { create } from "zustand";

import {
  ApiError,
  createMetaRepo as apiCreateMetaRepo,
  createRepo as apiCreateRepo,
  createSession as apiCreateSession,
  deleteMetaRepo as apiDeleteMetaRepo,
  deleteRepo as apiDeleteRepo,
  deleteSession as apiDeleteSession,
  deleteWorkspace as apiDeleteWorkspace,
  getAppState,
  renameRepo as apiRenameRepo,
  updateMetaRepo as apiUpdateMetaRepo,
  updateSession as apiUpdateSession,
  upgradeSession as apiUpgradeSession,
} from "../api/client";
import type {
  CreateRepoRequest,
  CreateSessionRequest,
  MetaRepoView,
  NodeView,
  PlanSummaryView,
  RepoView,
  SessionView,
  StatsResponse,
  UpdateSessionRequest,
  WorkspaceView,
} from "../api/types";
import {
  isSessionUnread,
  loadLastViewedMap,
  markLastViewed,
  saveLastViewedMap,
  type LastViewedMap,
} from "./useLastViewed";

const POLL_APP_STATE_MS = 3_000;
const REPO_EXPANSION_STORAGE_KEY = "sulion.sidebar.repoExpansion.v1";
const META_REPO_EXPANSION_STORAGE_KEY = "sulion.sidebar.metaRepoExpansion.v1";

type RepoExpansionMap = Record<string, boolean>;

let appStateRequest: Promise<void> | null = null;

export interface SaveMetaRepoInput {
  name: string;
  members: string[];
  primary_repo_name: string;
}

export interface SessionStore {
  nodes: NodeView[];
  sessions: SessionView[];
  repos: RepoView[];
  metaRepos: MetaRepoView[];
  workspaces: WorkspaceView[];
  stats: StatsResponse | null;
  plans: PlanSummaryView[];
  selectedSessionId: string | null;
  lastError: string | null;
  sessionsLoaded: boolean;
  lastViewed: LastViewedMap;
  repoExpansion: RepoExpansionMap;
  metaRepoExpansion: RepoExpansionMap;
  selectSession: (id: string | null) => void;
  createSession: (req: CreateSessionRequest) => Promise<SessionView>;
  deleteSession: (id: string) => Promise<void>;
  deleteWorkspace: (
    id: string,
    opts?: { force?: boolean; deleteBranch?: boolean },
  ) => Promise<void>;
  updateSession: (id: string, patch: UpdateSessionRequest) => Promise<void>;
  upgradeSession: (id: string) => Promise<void>;
  createRepo: (req: CreateRepoRequest) => Promise<RepoView>;
  createMetaRepo: (req: SaveMetaRepoInput) => Promise<MetaRepoView>;
  saveMetaRepo: (id: string, req: SaveMetaRepoInput) => Promise<MetaRepoView>;
  deleteMetaRepo: (id: string) => Promise<void>;
  renameRepo: (name: string, nextName: string) => Promise<RepoView>;
  deleteRepo: (name: string, opts?: { force?: boolean }) => Promise<void>;
  refresh: () => Promise<void>;
  isUnread: (sessionId: string, lastEventAt: string | null) => boolean;
  loadAppState: () => Promise<void>;
  setRepoExpanded: (repo: string, expanded: boolean) => void;
  setMetaRepoExpanded: (id: string, expanded: boolean) => void;
  collapseRepos: (repos: string[]) => void;
}

function initialState(): Pick<
  SessionStore,
  | "sessions"
  | "nodes"
  | "repos"
  | "metaRepos"
  | "workspaces"
  | "stats"
  | "plans"
  | "selectedSessionId"
  | "lastError"
  | "sessionsLoaded"
  | "lastViewed"
  | "repoExpansion"
  | "metaRepoExpansion"
> {
  return {
    nodes: [],
    sessions: [],
    repos: [],
    metaRepos: [],
    workspaces: [],
    stats: null,
    plans: [],
    selectedSessionId: readSessionIdFromUrl(),
    lastError: null,
    sessionsLoaded: false,
    lastViewed: loadLastViewedMap(),
    repoExpansion: loadRepoExpansionMap(),
    metaRepoExpansion: loadExpansionMap(META_REPO_EXPANSION_STORAGE_KEY),
  };
}

export const useSessionStore = create<SessionStore>()((set, get) => ({
  ...initialState(),

  async loadAppState() {
    if (!appStateRequest) {
      appStateRequest = (async () => {
        try {
          const data = await getAppState();
          const nodes = Array.isArray(data.nodes) ? data.nodes : [];
          const sessions = Array.isArray(data.sessions) ? data.sessions : [];
          const repos = Array.isArray(data.repos) ? data.repos : [];
          const metaRepos = Array.isArray(data.meta_repos) ? data.meta_repos : [];
          const workspaces = Array.isArray(data.workspaces) ? data.workspaces : [];
          const stats = data.stats ?? null;
          const plans = Array.isArray(data.plans) ? data.plans : [];
          set((state) => {
            const sameNodes = sameJson(state.nodes, nodes);
            const sameSessions = sameJson(state.sessions, sessions);
            const sameRepos = sameJson(state.repos, repos);
            const sameMetaRepos = sameJson(state.metaRepos, metaRepos);
            const sameWorkspaces = sameJson(state.workspaces, workspaces);
            const sameStats = sameJson(state.stats, stats);
            const samePlans = sameJson(state.plans, plans);
            if (
              sameNodes &&
              sameSessions &&
              sameRepos &&
              sameMetaRepos &&
              sameWorkspaces &&
              sameStats &&
              samePlans &&
              state.sessionsLoaded &&
              state.lastError == null
            ) {
              return state;
            }
            return {
              nodes: sameNodes ? state.nodes : nodes,
              sessions: sameSessions ? state.sessions : sessions,
              repos: sameRepos ? state.repos : repos,
              metaRepos: sameMetaRepos ? state.metaRepos : metaRepos,
              workspaces: sameWorkspaces ? state.workspaces : workspaces,
              stats: sameStats ? state.stats : stats,
              plans: samePlans ? state.plans : plans,
              lastError: null,
              sessionsLoaded: true,
            };
          });
        } catch (err) {
          console.error("getAppState failed", err);
          if (err instanceof ApiError) set({ lastError: err.message });
        }
      })();
    }

    const request = appStateRequest;
    try {
      await request;
    } finally {
      if (appStateRequest === request) appStateRequest = null;
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

  async deleteWorkspace(id, opts) {
    await apiDeleteWorkspace(id, opts);
    set((state) => ({
      workspaces: state.workspaces.filter((workspace) => workspace.id !== id),
      sessions: state.sessions.map((session) =>
        session.workspace?.id === id ? { ...session, workspace: null } : session,
      ),
    }));
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

  async upgradeSession(id) {
    try {
      await apiUpgradeSession(id);
    } catch (err) {
      if (err instanceof ApiError) set({ lastError: err.message });
      throw err;
    }
    // The shell was replaced under the same session id; refresh so the
    // sidebar and any attached panes see the restarted state promptly.
    await get().refresh();
  },

  async createRepo(req) {
    const created = await apiCreateRepo(req);
    set((state) => ({
      repos: [...state.repos, created].sort((a, b) => a.name.localeCompare(b.name)),
    }));
    return created;
  },

  async createMetaRepo(req) {
    const created = await apiCreateMetaRepo(req);
    set((state) => {
      const metaRepoExpansion = {
        ...state.metaRepoExpansion,
        [created.id]: true,
      };
      saveExpansionMap(META_REPO_EXPANSION_STORAGE_KEY, metaRepoExpansion);
      return {
        metaRepos: [...state.metaRepos, created].sort(metaRepoCompare),
        metaRepoExpansion,
      };
    });
    return created;
  },

  async saveMetaRepo(id, req) {
    const current = get().metaRepos.find((metaRepo) => metaRepo.id === id);
    if (!current) throw new Error("meta-repository no longer exists");
    const saved = await apiUpdateMetaRepo(id, {
      name: req.name,
      members: req.members,
      primary_repo_name: req.primary_repo_name,
    });

    set((state) => ({
      metaRepos: state.metaRepos
        .map((metaRepo) => (metaRepo.id === id ? saved : metaRepo))
        .sort(metaRepoCompare),
      sessions: state.sessions.map((session) =>
        session.meta_repo?.id === id
          ? { ...session, meta_repo: { id, name: saved.name } }
          : session,
      ),
    }));
    return saved;
  },

  async deleteMetaRepo(id) {
    await apiDeleteMetaRepo(id);
    set((state) => {
      const metaRepoExpansion = { ...state.metaRepoExpansion };
      delete metaRepoExpansion[id];
      saveExpansionMap(META_REPO_EXPANSION_STORAGE_KEY, metaRepoExpansion);
      return {
        metaRepos: state.metaRepos.filter((metaRepo) => metaRepo.id !== id),
        metaRepoExpansion,
      };
    });
  },

  async renameRepo(name, nextName) {
    const existing = get().repos.find((repo) => repo.name === name);
    const renamed = await apiRenameRepo(name, { name: nextName });
    set((state) => {
      const repoExpansion = migrateRepoExpansion(
        state.repoExpansion,
        name,
        renamed.name,
      );
      const oldPath = existing?.path ?? "";
      const newPath = renamed.path;
      return {
        repos: [
          ...state.repos.filter(
            (repo) => repo.name !== name && repo.name !== renamed.name,
          ),
          renamed,
        ].sort((a, b) => a.name.localeCompare(b.name)),
        sessions: state.sessions.map((session) =>
          session.repo === name
            ? {
                ...session,
                repo: renamed.name,
                working_dir: replacePathPrefix(session.working_dir, oldPath, newPath),
                workspace:
                  session.workspace?.repo_name === name
                    ? {
                        ...session.workspace,
                        repo_name: renamed.name,
                        path: replacePathPrefix(session.workspace.path, oldPath, newPath),
                      }
                    : session.workspace,
              }
            : session,
        ),
        metaRepos: state.metaRepos.map((metaRepo) => ({
          ...metaRepo,
          primary_repo_name:
            metaRepo.primary_repo_name === name
              ? renamed.name
              : metaRepo.primary_repo_name,
          members: metaRepo.members.map((member) =>
            member.repo_name === name
              ? { ...member, repo_name: renamed.name }
              : member,
          ),
        })),
        workspaces: state.workspaces.map((workspace) =>
          workspace.repo_name === name
            ? {
                ...workspace,
                repo_name: renamed.name,
                path: replacePathPrefix(workspace.path, oldPath, newPath),
              }
            : workspace,
        ),
        repoExpansion,
      };
    });
    return renamed;
  },

  async deleteRepo(name, opts) {
    await apiDeleteRepo(name, opts);
    set((state) => ({
      repos: state.repos.filter((repo) => repo.name !== name),
      metaRepos: state.metaRepos.map((metaRepo) => {
        const members = metaRepo.members.filter(
          (member) => member.repo_name !== name,
        );
        return {
          ...metaRepo,
          members,
          primary_repo_name:
            metaRepo.primary_repo_name === name
              ? (members[0]?.repo_name ?? null)
              : metaRepo.primary_repo_name,
        };
      }),
    }));
  },

  async refresh() {
    await get().loadAppState();
  },

  isUnread(sessionId, lastEventAt) {
    return isSessionUnread(get().lastViewed, sessionId, lastEventAt);
  },

  setRepoExpanded(repo, expanded) {
    set((state) => {
      const repoExpansion = { ...state.repoExpansion, [repo]: expanded };
      saveRepoExpansionMap(repoExpansion);
      return { repoExpansion };
    });
  },

  setMetaRepoExpanded(id, expanded) {
    set((state) => {
      const metaRepoExpansion = { ...state.metaRepoExpansion, [id]: expanded };
      saveExpansionMap(META_REPO_EXPANSION_STORAGE_KEY, metaRepoExpansion);
      return { metaRepoExpansion };
    });
  },

  collapseRepos(repos) {
    set((state) => {
      const repoExpansion = { ...state.repoExpansion };
      for (const repo of repos) {
        repoExpansion[repo] = false;
      }
      saveRepoExpansionMap(repoExpansion);
      return { repoExpansion };
    });
  },
}));

let consumerCount = 0;
let appStateTimer: ReturnType<typeof setTimeout> | null = null;
let pollingGeneration = 0;
let popstateAttached = false;

function syncSelectedSessionFromUrl() {
  useSessionStore.setState({ selectedSessionId: readSessionIdFromUrl() });
}

function startSessionStore() {
  if (typeof window === "undefined") return;
  consumerCount += 1;
  if (consumerCount > 1) return;

  const generation = ++pollingGeneration;
  const poll = async () => {
    await useSessionStore.getState().loadAppState();
    if (consumerCount === 0 || pollingGeneration !== generation) return;
    appStateTimer = window.setTimeout(() => {
      appStateTimer = null;
      void poll();
    }, POLL_APP_STATE_MS);
  };
  void poll();
  if (!popstateAttached) {
    window.addEventListener("popstate", syncSelectedSessionFromUrl);
    popstateAttached = true;
  }
}

function stopSessionStore() {
  if (typeof window === "undefined") return;
  consumerCount = Math.max(0, consumerCount - 1);
  if (consumerCount > 0) return;

  pollingGeneration += 1;
  if (appStateTimer) {
    clearTimeout(appStateTimer);
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
  pollingGeneration += 1;
  appStateRequest = null;
  if (appStateTimer) {
    clearTimeout(appStateTimer);
    appStateTimer = null;
  }
  if (typeof window !== "undefined" && popstateAttached) {
    window.removeEventListener("popstate", syncSelectedSessionFromUrl);
  }
  popstateAttached = false;
  useSessionStore.setState(initialState());
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

function sameJson(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

function loadRepoExpansionMap(): RepoExpansionMap {
  return loadExpansionMap(REPO_EXPANSION_STORAGE_KEY);
}

function loadExpansionMap(storageKey: string): RepoExpansionMap {
  if (typeof window === "undefined") return {};
  try {
    const raw = window.localStorage.getItem(storageKey);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: RepoExpansionMap = {};
    for (const [key, value] of Object.entries(parsed)) {
      if (typeof key === "string" && typeof value === "boolean") {
        out[key] = value;
      }
    }
    return out;
  } catch {
    return {};
  }
}

function saveRepoExpansionMap(map: RepoExpansionMap) {
  saveExpansionMap(REPO_EXPANSION_STORAGE_KEY, map);
}

function saveExpansionMap(storageKey: string, map: RepoExpansionMap) {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(storageKey, JSON.stringify(map));
  } catch {
    /* ignore */
  }
}

function metaRepoCompare(a: MetaRepoView, b: MetaRepoView): number {
  return a.name.localeCompare(b.name);
}

function migrateRepoExpansion(
  map: RepoExpansionMap,
  from: string,
  to: string,
): RepoExpansionMap {
  if (from === to || !Object.prototype.hasOwnProperty.call(map, from)) return map;
  const next = { ...map, [to]: map[from] };
  delete next[from];
  saveRepoExpansionMap(next);
  return next;
}

function replacePathPrefix(value: string, fromPrefix: string, toPrefix: string): string {
  if (!fromPrefix || !toPrefix) return value;
  if (value === fromPrefix) return toPrefix;
  const prefix = `${fromPrefix}/`;
  if (value.startsWith(prefix)) {
    return `${toPrefix}${value.slice(fromPrefix.length)}`;
  }
  return value;
}
