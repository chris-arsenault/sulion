// Tab registry. Thin by design: per-tab identity (kind + refs) plus the
// pane membership + active state. Nothing about what a tab is
// internally *doing* lives here — terminal scroll, timeline filters,
// search query, etc. belong to the tab component itself and die with
// its mount.
//
// Architectural principle: each tab is its own subtree with its own
// state, analogous to a separate process on a desktop app. The
// registry only routes; the tabs run. Outside writers (sidebar click,
// Cmd-K, keyboard shortcut) dispatch against the registry via
// `openTab` / `closeTab`; they do not subscribe to tab internals.

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

const STORAGE_KEY = "shuttlecraft.tabs.v2";

export type PaneId = "top" | "bottom";
export type TabKind = "terminal" | "timeline" | "file" | "diff" | "search";

/** Minimal registry entry. Kind + refs is the identity; everything
 * else is the tab's own business. */
export interface TabData {
  id: string;
  kind: TabKind;
  /** For session-bound tabs (terminal, timeline). */
  sessionId?: string;
  /** For repo-bound tabs (file, diff). */
  repo?: string;
  /** File path for file/diff tabs; optional for diff (whole-repo). */
  path?: string;
}

export interface TabStore {
  tabs: Record<string, TabData>;
  panes: Record<PaneId, string[]>;
  activeByPane: Record<PaneId, string | null>;
  openTab: (spec: Omit<TabData, "id">, pane?: PaneId) => string;
  closeTab: (id: string) => void;
  activateTab: (pane: PaneId, id: string) => void;
  /** Move a tab to a different pane (or reorder within one). `index` is
   * the position in the target pane's list; if omitted, appends. */
  moveTab: (id: string, toPane: PaneId, index?: number) => void;
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

  // Hydrate from localStorage once, self-healing invariant violations
  // that older builds may have persisted.
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
      const hTabs: Record<string, TabData> = {};
      for (const [id, t] of Object.entries(parsed.tabs ?? {})) {
        if (!t || typeof t !== "object") continue;
        // Only persist the minimal fields. If older storage had
        // additional keys (searchQuery, title, etc.) they're dropped.
        hTabs[id] = {
          id: t.id ?? id,
          kind: t.kind,
          sessionId: t.sessionId,
          repo: t.repo,
          path: t.path,
        } as TabData;
      }
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
      // Corrupt storage — reset.
    }
  }, []);

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
    (spec: Omit<TabData, "id">, pane: PaneId = defaultPaneFor(spec.kind)) => {
      const key = tabKey(spec);
      const existingId = Object.keys(tabs).find((id) => tabKey(tabs[id]!) === key);
      if (existingId) {
        const inPane: PaneId | null =
          (["top", "bottom"] as PaneId[]).find((p) => panes[p].includes(existingId)) ??
          null;
        if (inPane) {
          setActiveByPane((prev) => ({ ...prev, [inPane]: existingId }));
        }
        return existingId;
      }
      const id = crypto.randomUUID();
      const data: TabData = { ...spec, id };
      setTabs((prev) => ({ ...prev, [id]: data }));
      setPanes((prev) => ({ ...prev, [pane]: [...prev[pane], id] }));
      setActiveByPane((prev) => ({ ...prev, [pane]: id }));
      return id;
    },
    [panes, tabs],
  );

  const closeTab = useCallback(
    (id: string) => {
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
            if (paneList.length === 0) {
              next[p] = null;
            } else {
              const oldIdx = (panes[p] ?? []).indexOf(id);
              if (oldIdx > 0 && paneList[oldIdx - 1]) {
                next[p] = paneList[oldIdx - 1]!;
              } else {
                next[p] = paneList[paneList.length - 1]!;
              }
            }
          } else if (next[p] != null && !paneList.includes(next[p]!)) {
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
    },
    [panes],
  );

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
      const at =
        index == null || index < 0 || index > targetList.length
          ? targetList.length
          : index;
      targetList.splice(at, 0, id);
      next[toPane] = targetList;
      return next;
    });
    setActiveByPane((prev) => ({ ...prev, [toPane]: id }));
  }, []);

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
      hasAnyTab,
    }),
    [tabs, panes, activeByPane, openTab, closeTab, activateTab, moveTab, hasAnyTab],
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

function defaultPaneFor(kind: TabKind): PaneId {
  return kind === "timeline" ? "bottom" : "top";
}
