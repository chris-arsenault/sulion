import { useEffect } from "react";
import { create } from "zustand";

import { getRepoFiles, getRepoGit } from "../api/client";
import type { DirListing, GitStatus } from "../api/types";

const POLL_EXPANDED_MS = 5_000;
const POLL_COLLAPSED_MS = 60_000;

export interface RepoState {
  git: GitStatus | null;
  gitLastFetched: number;
  gitError: string | null;
  /** path -> listing. Missing key = not loaded. Null value = loading. */
  tree: Record<string, DirListing | null>;
  /** User-expanded directories (repo-relative paths). Root is "". */
  expanded: Set<string>;
  /** User-collapsed directories. Wins over auto-expand-on-dirty. */
  collapsed: Set<string>;
  /** Show ignored/untracked-by-gitignore files in listings. */
  showAll: boolean;
}

export interface RepoStore {
  repos: Record<string, RepoState>;
  expandedRepos: Set<string>;
  setExpanded: (repo: string, expanded: boolean) => void;
  toggleDir: (repo: string, path: string, currentlyExpanded: boolean) => void;
  refresh: (repo: string) => void;
  setShowAll: (repo: string, value: boolean) => void;
  loadDir: (repo: string, path: string) => void;
  pollOne: (repo: string) => Promise<void>;
}

function createRepoState(): RepoState {
  return {
    git: null,
    gitLastFetched: 0,
    gitError: null,
    tree: {},
    expanded: new Set(),
    collapsed: new Set(),
    showAll: false,
  };
}

function initialState(): Pick<RepoStore, "repos" | "expandedRepos"> {
  return {
    repos: {},
    expandedRepos: new Set(),
  };
}

const knownRepoNames = new Set<string>();

export const useRepoStore = create<RepoStore>()((set, get) => ({
  ...initialState(),

  async pollOne(name) {
    knownRepoNames.add(name);
    try {
      const git = await getRepoGit(name);
      set((state) => ({
        repos: {
          ...state.repos,
          [name]: {
            ...(state.repos[name] ?? createRepoState()),
            git,
            gitLastFetched: Date.now(),
            gitError: null,
          },
        },
      }));
    } catch (err) {
      const msg = err instanceof Error ? err.message : "unknown";
      set((state) => ({
        repos: {
          ...state.repos,
          [name]: {
            ...(state.repos[name] ?? createRepoState()),
            gitError: msg,
          },
        },
      }));
    }
  },

  setExpanded(repo, expanded) {
    knownRepoNames.add(repo);
    set((state) => ({
      repos: {
        ...state.repos,
        [repo]: state.repos[repo] ?? createRepoState(),
      },
      expandedRepos: mutateSet(state.expandedRepos, (next) => {
        if (expanded) next.add(repo);
        else next.delete(repo);
      }),
    }));
    if (expanded) void get().pollOne(repo);
  },

  toggleDir(repo, path, currentlyExpanded) {
    set((state) => {
      const current = state.repos[repo];
      if (!current) return state;
      const expanded = new Set(current.expanded);
      const collapsed = new Set(current.collapsed);
      if (currentlyExpanded) {
        expanded.delete(path);
        collapsed.add(path);
      } else {
        collapsed.delete(path);
        expanded.add(path);
      }
      return {
        repos: {
          ...state.repos,
          [repo]: { ...current, expanded, collapsed },
        },
      };
    });
  },

  refresh(repo) {
    knownRepoNames.add(repo);
    set((state) => ({
      repos: {
        ...state.repos,
        [repo]: {
          ...(state.repos[repo] ?? createRepoState()),
          tree: {},
        },
      },
    }));
    void get().pollOne(repo);
  },

  setShowAll(repo, value) {
    set((state) => {
      const current = state.repos[repo];
      if (!current) return state;
      return {
        repos: {
          ...state.repos,
          [repo]: { ...current, showAll: value, tree: {} },
        },
      };
    });
  },

  loadDir(repo, path) {
    knownRepoNames.add(repo);
    set((state) => ({
      repos: {
        ...state.repos,
        [repo]: state.repos[repo] ?? createRepoState(),
      },
    }));

    const current = get().repos[repo];
    if (current && current.tree[path] !== undefined) return;

    set((state) => ({
      repos: {
        ...state.repos,
        [repo]: {
          ...(state.repos[repo] ?? createRepoState()),
          tree: {
            ...(state.repos[repo]?.tree ?? {}),
            [path]: null,
          },
        },
      },
    }));

    void (async () => {
      try {
        const showAll = get().repos[repo]?.showAll ?? false;
        const listing = await getRepoFiles(repo, path, showAll);
        set((state) => ({
          repos: {
            ...state.repos,
            [repo]: {
              ...(state.repos[repo] ?? createRepoState()),
              tree: {
                ...(state.repos[repo]?.tree ?? {}),
                [path]: listing,
              },
            },
          },
        }));
      } catch {
        // Silent — tree rows render an error badge from missing entries.
      }
    })();
  },
}));

let consumerCount = 0;
let pollTimer: ReturnType<typeof setInterval> | null = null;

function startRepoStore() {
  if (typeof window === "undefined") return;
  consumerCount += 1;
  if (consumerCount > 1) return;

  pollTimer = window.setInterval(() => {
    if (typeof document !== "undefined" && document.hidden) return;
    const { repos, expandedRepos, pollOne } = useRepoStore.getState();
    const now = Date.now();
    for (const name of knownRepoNames) {
      const repoState = repos[name];
      const age = now - (repoState?.gitLastFetched ?? 0);
      const cadence = expandedRepos.has(name) ? POLL_EXPANDED_MS : POLL_COLLAPSED_MS;
      if (age >= cadence) void pollOne(name);
    }
  }, 1_000);
}

function stopRepoStore() {
  consumerCount = Math.max(0, consumerCount - 1);
  if (consumerCount > 0) return;

  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

export function useRepos<T>(selector: (state: RepoStore) => T): T {
  useEffect(() => {
    startRepoStore();
    return stopRepoStore;
  }, []);
  return useRepoStore(selector);
}

export function resetRepoStore() {
  consumerCount = 0;
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
  knownRepoNames.clear();
  useRepoStore.setState(initialState());
}

function mutateSet<T>(input: Set<T>, update: (next: Set<T>) => void): Set<T> {
  const next = new Set(input);
  update(next);
  return next;
}

/** Walk `dirty_by_path` and return the set of ancestor directories
 * that contain any dirty descendant. Used to auto-expand the tree. */
export function dirtyAncestors(dirtyByPath: Record<string, string>): Set<string> {
  const out = new Set<string>();
  for (const path of Object.keys(dirtyByPath)) {
    let cur = path;
    while (true) {
      const idx = cur.lastIndexOf("/");
      if (idx === -1) break;
      cur = cur.slice(0, idx);
      out.add(cur);
    }
  }
  return out;
}

/** Staleness classification for a repo's header badge. */
export function stalenessFor(
  git: GitStatus | null,
  latestEventAt: number | null,
): "green" | "amber" | "red" {
  if (!git || git.uncommitted_count === 0) return "green";
  const lastCommitMs = git.last_commit
    ? new Date(git.last_commit.committed_at).getTime()
    : 0;
  if (latestEventAt == null || latestEventAt <= lastCommitMs) return "green";
  const gap = latestEventAt - lastCommitMs;
  if (gap > 15 * 60 * 1000) return "red";
  return "amber";
}
