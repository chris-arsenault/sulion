// Heterogeneous tab state for the two-pane work area. Tabs are
// (kind, ref) tuples; panes are ordered tab-id lists with one active
// tab per pane. Tabs are de-duplicated by key so selecting an already-
// open session just raises its existing tabs.
//
// State persists to localStorage and re-hydrates on mount, minus any
// tab whose underlying session no longer exists (sessions get garbage-
// collected when the PTY is deleted).

import {
  type ReactNode,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

const STORAGE_KEY = "shuttlecraft.tabs.v1";

export type PaneId = "top" | "bottom";
export type TabKind = "terminal" | "timeline" | "file" | "diff" | "search";

export interface TabData {
  id: string;
  kind: TabKind;
  /** For session-bound tabs. */
  sessionId?: string;
  /** For repo-bound tabs. */
  repo?: string;
  /** File path for file/diff tabs; optional for diff (whole-repo). */
  path?: string;
  /** Search-tab initial state. */
  searchQuery?: string;
  searchScope?: "timeline" | "repo" | "workspace";
  /** Shown on the tab label. Derived from kind + ref. */
  title: string;
}

export interface TabStore {
  tabs: Record<string, TabData>;
  panes: Record<PaneId, string[]>;
  activeByPane: Record<PaneId, string | null>;
  openTab: (spec: Omit<TabData, "id" | "title">, pane?: PaneId) => string;
  closeTab: (id: string) => void;
  activateTab: (pane: PaneId, id: string) => void;
  /** Move a tab to a different pane (or reorder within one). `index` is
   * the position in the target pane's list; if omitted, appends. */
  moveTab: (id: string, toPane: PaneId, index?: number) => void;
  /** Drop all tabs that reference a session that no longer exists. */
  pruneSessions: (liveSessionIds: Set<string>) => void;
  /** True when at least one tab is open in either pane. */
  hasAnyTab: boolean;
}

const Ctx = createContext<TabStore | null>(null);

export function TabProvider({ children }: { children: ReactNode }) {
  const [tabs, setTabs] = useState<Record<string, TabData>>({});
  const [panes, setPanes] = useState<Record<PaneId, string[]>>({
    top: [],
    bottom: [],
  });
  const [activeByPane, setActiveByPane] = useState<Record<PaneId, string | null>>({
    top: null,
    bottom: null,
  });
  const hydrated = useRef(false);

  // Hydrate from localStorage once. Self-heals a couple of invariant
  // violations that older builds may have persisted:
  //   - active id that doesn't exist in the pane → pick the last tab
  //   - pane has tabs but activeByPane is null → pick the last tab
  //   - active id references a tab that doesn't exist in `tabs` → clear
  // Result: a hydrated pane with tabs is guaranteed to have a valid
  // activeId. This is the "refresh and content is blank" fix.
  useEffect(() => {
    if (hydrated.current) return;
    hydrated.current = true;
    try {
      const raw = localStorage.getItem(STORAGE_KEY);
      if (!raw) return;
      const parsed = JSON.parse(raw) as Partial<{
        tabs: Record<string, TabData>;
        panes: Record<PaneId, string[]>;
        activeByPane: Record<PaneId, string | null>;
      }>;
      const hTabs = parsed.tabs ?? {};
      const hPanes: Record<PaneId, string[]> = {
        top: (parsed.panes?.top ?? []).filter((id) => id in hTabs),
        bottom: (parsed.panes?.bottom ?? []).filter((id) => id in hTabs),
      };
      const pickActive = (pane: PaneId): string | null => {
        const ids = hPanes[pane];
        if (ids.length === 0) return null;
        const persisted = parsed.activeByPane?.[pane];
        if (persisted && ids.includes(persisted)) return persisted;
        return ids[ids.length - 1]!;
      };
      setTabs(hTabs);
      setPanes(hPanes);
      setActiveByPane({
        top: pickActive("top"),
        bottom: pickActive("bottom"),
      });
    } catch {
      // Corrupt storage — reset by not hydrating.
    }
  }, []);

  // Persist on any change. Cheap — small object.
  useEffect(() => {
    if (!hydrated.current) return;
    try {
      localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({ tabs, panes, activeByPane }),
      );
    } catch {
      // Quota or disabled storage — drop silently.
    }
  }, [tabs, panes, activeByPane]);

  const openTab = useCallback(
    (spec: Omit<TabData, "id" | "title">, pane: PaneId = defaultPaneFor(spec.kind)) => {
      const key = tabKey(spec);
      // De-dupe: existing tab with the same key gets raised.
      const existingId = Object.keys(tabs).find((id) => tabKey(tabs[id]!) === key);
      if (existingId) {
        // Raise it in whichever pane it's in.
        const inPane: PaneId | null =
          (["top", "bottom"] as PaneId[]).find((p) => panes[p].includes(existingId)) ?? null;
        if (inPane) {
          setActiveByPane((prev) => ({ ...prev, [inPane]: existingId }));
        }
        return existingId;
      }
      const id = crypto.randomUUID();
      const title = deriveTitle(spec);
      const data: TabData = { ...spec, id, title };
      setTabs((prev) => ({ ...prev, [id]: data }));
      setPanes((prev) => ({ ...prev, [pane]: [...prev[pane], id] }));
      setActiveByPane((prev) => ({ ...prev, [pane]: id }));
      return id;
    },
    [panes, tabs],
  );

  const closeTab = useCallback((id: string) => {
    // Single-pass update: if we close the active tab in a pane, promote
    // the neighbour (prev sibling, else next) instead of leaving
    // activeId=null with live tabs still in the pane — that would
    // render an empty content area even though the strip shows tabs.
    let newTop: string[] | null = null;
    let newBottom: string[] | null = null;
    setPanes((prev) => {
      newTop = prev.top.filter((t) => t !== id);
      newBottom = prev.bottom.filter((t) => t !== id);
      return { top: newTop, bottom: newBottom };
    });
    setActiveByPane((prev) => {
      const next = { ...prev };
      for (const p of ["top", "bottom"] as PaneId[]) {
        const paneList = (p === "top" ? newTop : newBottom) ?? [];
        if (next[p] === id) {
          // Promote the closed tab's neighbour: prefer the one that was
          // immediately before it (easier to re-find on screen), fall
          // back to the last remaining tab.
          const oldIdx = (panes[p] ?? []).indexOf(id);
          if (paneList.length === 0) {
            next[p] = null;
          } else if (oldIdx > 0 && paneList[oldIdx - 1]) {
            next[p] = paneList[oldIdx - 1]!;
          } else {
            next[p] = paneList[paneList.length - 1]!;
          }
        } else if (next[p] != null && !paneList.includes(next[p]!)) {
          // Belt-and-braces: if active id somehow drifted out of the
          // pane's list, reset.
          next[p] = paneList[paneList.length - 1] ?? null;
        }
      }
      return next;
    });
    setTabs((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
  }, [panes]);

  const activateTab = useCallback((pane: PaneId, id: string) => {
    setActiveByPane((prev) => ({ ...prev, [pane]: id }));
  }, []);

  const moveTab = useCallback((id: string, toPane: PaneId, index?: number) => {
    setPanes((prev) => {
      const currentPane: PaneId | null =
        (["top", "bottom"] as PaneId[]).find((p) => prev[p].includes(id)) ?? null;
      if (!currentPane) return prev;
      const next = {
        top: [...prev.top],
        bottom: [...prev.bottom],
      };
      next[currentPane] = next[currentPane].filter((t) => t !== id);
      const targetList = next[toPane];
      const at = index == null || index < 0 || index > targetList.length
        ? targetList.length
        : index;
      targetList.splice(at, 0, id);
      next[toPane] = targetList;
      return next;
    });
    setActiveByPane((prev) => ({ ...prev, [toPane]: id }));
  }, []);

  const pruneSessions = useCallback(
    (live: Set<string>) => {
      const doomed = Object.values(tabs)
        .filter(
          (t) =>
            (t.kind === "terminal" || t.kind === "timeline") &&
            t.sessionId &&
            !live.has(t.sessionId),
        )
        .map((t) => t.id);
      for (const id of doomed) closeTab(id);
    },
    [tabs, closeTab],
  );

  const hasAnyTab = panes.top.length > 0 || panes.bottom.length > 0;

  const value: TabStore = useMemo(
    () => ({
      tabs,
      panes,
      activeByPane,
      openTab,
      closeTab,
      activateTab,
      moveTab,
      pruneSessions,
      hasAnyTab,
    }),
    [tabs, panes, activeByPane, openTab, closeTab, activateTab, moveTab, pruneSessions, hasAnyTab],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useTabs(): TabStore {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useTabs called outside TabProvider");
  return ctx;
}

/** Canonical key that de-duplicates a tab spec. */
export function tabKey(spec: Pick<TabData, "kind" | "sessionId" | "repo" | "path">): string {
  switch (spec.kind) {
    case "terminal":
    case "timeline":
      return `${spec.kind}:${spec.sessionId ?? ""}`;
    case "file":
      return `file:${spec.repo ?? ""}:${spec.path ?? ""}`;
    case "diff":
      return `diff:${spec.repo ?? ""}:${spec.path ?? ""}`;
    case "search":
      return "search"; // single search tab
  }
}

function deriveTitle(spec: Pick<TabData, "kind" | "sessionId" | "repo" | "path">): string {
  switch (spec.kind) {
    case "terminal":
      return "terminal";
    case "timeline":
      return "timeline";
    case "file":
      return spec.path ? basename(spec.path) : "file";
    case "diff":
      return spec.path ? `diff: ${basename(spec.path)}` : "diff";
    case "search":
      return "search";
  }
}

function defaultPaneFor(kind: TabKind): PaneId {
  // Terminal wants the top slot; timeline the bottom. Everything else
  // defaults to top so it's immediately visible.
  return kind === "timeline" ? "bottom" : "top";
}

function basename(p: string): string {
  const i = p.lastIndexOf("/");
  return i === -1 ? p : p.slice(i + 1);
}
