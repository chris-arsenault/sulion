import { create } from "zustand";

import { getRepoDirtyPaths, getRepoFiles, refreshRepoState } from "../api/client";
import type { DirListing, GitStatus, RepoGitSummary } from "../api/types";

export interface RepoState {
  git: GitStatus | null;
  dirtyLoadedRevision: number | null;
  gitError: string | null;
  /** path -> listing. Missing key = not loaded. Null value = loading. */
  tree: Record<string, DirListing | null>;
  /** Monotonic invalidation token for in-flight tree requests. */
  treeEpoch: number;
  /** User-expanded directories (repo-relative paths). Root is "". */
  expanded: Set<string>;
  /** User-collapsed directories. Wins over auto-expand-on-dirty. */
  collapsed: Set<string>;
  /** Show ignored/untracked-by-gitignore files in listings. */
  showAll: boolean;
}

export interface RepoStore {
  repos: Record<string, RepoState>;
  loadDirty: (repo: string, summary: RepoGitSummary | null | undefined) => void;
  toggleDir: (repo: string, path: string, currentlyExpanded: boolean) => void;
  expandPath: (repo: string, path: string) => void;
  refresh: (repo: string) => void;
  hardRefresh: (repo: string) => void;
  setShowAll: (repo: string, value: boolean) => void;
  loadDir: (repo: string, path: string, opts?: { force?: boolean }) => void;
  refreshVisibleDirs: (repo: string, opts?: { clear?: boolean }) => void;
}

function createRepoState(): RepoState {
  return {
    git: null,
    dirtyLoadedRevision: null,
    gitError: null,
    tree: {},
    treeEpoch: 0,
    expanded: new Set(),
    collapsed: new Set(),
    showAll: false,
  };
}

function initialState(): Pick<RepoStore, "repos"> {
  return {
    repos: {},
  };
}

export const useRepoStore = create<RepoStore>()((set, get) => ({
  ...initialState(),

  loadDirty(repo, summary) {
    if (!summary) {
      set((state) => ({
        repos: {
          ...state.repos,
          [repo]: {
            ...(state.repos[repo] ?? createRepoState()),
            git: null,
            dirtyLoadedRevision: null,
            gitError: null,
          },
        },
      }));
      return;
    }
    const current = get().repos[repo];
    if (current?.dirtyLoadedRevision === summary.revision) return;
    set((state) => ({
      repos: {
        ...state.repos,
        [repo]: {
          ...(state.repos[repo] ?? createRepoState()),
          git: gitFromSummary(summary),
          gitError: null,
        },
      },
    }));
    void (async () => {
      try {
        const dirty = await getRepoDirtyPaths(repo);
        if (dirty.git_revision !== summary.revision) return;
        set((state) => ({
          repos: {
            ...state.repos,
            [repo]: {
              ...(state.repos[repo] ?? createRepoState()),
              git: gitFromSummary(summary, dirty.dirty_by_path, dirty.diff_stats_by_path),
              dirtyLoadedRevision: summary.revision,
              gitError: null,
            },
          },
        }));
        get().refreshVisibleDirs(repo);
      } catch (err) {
        const msg = err instanceof Error ? err.message : "unknown";
        set((state) => ({
          repos: {
            ...state.repos,
            [repo]: {
              ...(state.repos[repo] ?? createRepoState()),
              gitError: msg,
            },
          },
        }));
      }
    })();
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

  expandPath(repo, path) {
    const dirs = ancestorDirs(path);
    set((state) => {
      const current = state.repos[repo] ?? createRepoState();
      const expanded = new Set(current.expanded);
      const collapsed = new Set(current.collapsed);
      for (const dir of dirs) {
        expanded.add(dir);
        collapsed.delete(dir);
      }
      return {
        repos: {
          ...state.repos,
          [repo]: { ...current, expanded, collapsed },
        },
      };
    });
    for (const dir of ["", ...dirs]) {
      get().loadDir(repo, dir);
    }
  },

  refresh(repo) {
    get().refreshVisibleDirs(repo);
    void refreshRepoState(repo).catch(() => {});
  },

  hardRefresh(repo) {
    get().refreshVisibleDirs(repo, { clear: true });
    void refreshRepoState(repo).catch(() => {});
  },

  setShowAll(repo, value) {
    set((state) => {
      const current = state.repos[repo];
      if (!current) return state;
      return {
        repos: {
          ...state.repos,
          [repo]: {
            ...current,
            showAll: value,
            tree: {},
            treeEpoch: current.treeEpoch + 1,
          },
        },
      };
    });
    get().refreshVisibleDirs(repo);
  },

  loadDir(repo, path, opts) {
    set((state) => ({
      repos: {
        ...state.repos,
        [repo]: state.repos[repo] ?? createRepoState(),
      },
    }));

    const current = get().repos[repo];
    if (!opts?.force && current && current.tree[path] !== undefined) return;
    const epoch = current?.treeEpoch ?? 0;

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
        set((state) => {
          if ((state.repos[repo]?.treeEpoch ?? 0) !== epoch) {
            return state;
          }
          return {
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
          };
        });
      } catch {
        // Silent — tree rows render an error badge from missing entries.
      }
    })();
  },

  refreshVisibleDirs(repo, opts) {
    const current = get().repos[repo] ?? createRepoState();
    const paths = visibleDirPaths(current);
    if (opts?.clear) {
      set((state) => ({
        repos: {
          ...state.repos,
          [repo]: {
            ...(state.repos[repo] ?? createRepoState()),
            tree: {},
            treeEpoch: (state.repos[repo]?.treeEpoch ?? 0) + 1,
          },
        },
      }));
    }
    for (const path of paths) {
      get().loadDir(repo, path, { force: true });
    }
  },
}));

export function useRepos<T>(selector: (state: RepoStore) => T): T {
  return useRepoStore(selector);
}

export function resetRepoStore() {
  useRepoStore.setState(initialState());
}

function ancestorDirs(path: string): string[] {
  const parts = path.split("/").filter(Boolean);
  const dirs: string[] = [];
  for (let i = 1; i < parts.length; i += 1) {
    dirs.push(parts.slice(0, i).join("/"));
  }
  return dirs;
}

function visibleDirPaths(state: RepoState): string[] {
  const paths = new Set<string>([""]);
  for (const path of state.expanded) paths.add(path);
  for (const path of Object.keys(state.tree)) paths.add(path);
  return [...paths].sort((a, b) => a.localeCompare(b));
}

function gitFromSummary(
  summary: RepoGitSummary,
  dirty_by_path: GitStatus["dirty_by_path"] = {},
  diff_stats_by_path: GitStatus["diff_stats_by_path"] = {},
): GitStatus {
  return {
    branch: summary.branch,
    uncommitted_count: summary.uncommitted_count,
    untracked_count: summary.untracked_count,
    last_commit: summary.last_commit,
    recent_commits: summary.recent_commits,
    dirty_by_path,
    diff_stats_by_path,
  };
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
  git: Pick<GitStatus, "last_commit" | "uncommitted_count"> | null,
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
