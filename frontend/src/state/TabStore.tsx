// Heterogeneous tab state for the two-pane work area. Tabs are
// (kind, ref) tuples; panes are ordered tab-id lists with one active
// tab per pane. Tabs are de-duplicated by key so selecting an already-
// open session just raises its existing tabs.
//
// Implementation: Zustand with the persist middleware. This is the
// first store we migrated off of React context — see docs/state.md
// for the decision rationale. Consumer API (`useTabs()`) is unchanged
// so call sites didn't need edits; new consumers can use
// `useTabStore(selector)` for fine-grained re-render control.

import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";

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

interface PersistedShape {
  tabs: Record<string, TabData>;
  panes: Record<PaneId, string[]>;
  activeByPane: Record<PaneId, string | null>;
}

export const useTabStore = create<TabStore>()(
  persist(
    (set, get) => ({
      tabs: {},
      panes: { top: [], bottom: [] },
      activeByPane: { top: null, bottom: null },
      // `hasAnyTab` is derived; we recompute on every mutation that could
      // change it. Zustand selectors on consumers make this cheap either way.
      hasAnyTab: false,

      openTab: (spec, pane = defaultPaneFor(spec.kind)) => {
        const { tabs, panes, activeByPane } = get();
        const key = tabKey(spec);
        const existingId = Object.keys(tabs).find((id) => tabKey(tabs[id]!) === key);
        if (existingId) {
          const inPane: PaneId | null =
            (["top", "bottom"] as PaneId[]).find((p) => panes[p].includes(existingId)) ??
            null;
          if (inPane) {
            set({ activeByPane: { ...activeByPane, [inPane]: existingId } });
          }
          return existingId;
        }
        const id = crypto.randomUUID();
        const title = deriveTitle(spec);
        const data: TabData = { ...spec, id, title };
        const newTabs = { ...tabs, [id]: data };
        const newPanes = { ...panes, [pane]: [...panes[pane], id] };
        set({
          tabs: newTabs,
          panes: newPanes,
          activeByPane: { ...activeByPane, [pane]: id },
          hasAnyTab: newPanes.top.length > 0 || newPanes.bottom.length > 0,
        });
        return id;
      },

      closeTab: (id) => {
        const { tabs, panes, activeByPane } = get();
        // Single-pass: if we close the active tab in a pane, promote a
        // neighbour (prev sibling, else last remaining). Prevents the
        // "strip shows tabs but content is empty" invariant violation.
        const newPanes: Record<PaneId, string[]> = {
          top: panes.top.filter((t) => t !== id),
          bottom: panes.bottom.filter((t) => t !== id),
        };
        const newActive: Record<PaneId, string | null> = { ...activeByPane };
        for (const p of ["top", "bottom"] as PaneId[]) {
          const oldList = panes[p];
          const newList = newPanes[p];
          if (newActive[p] === id) {
            if (newList.length === 0) {
              newActive[p] = null;
            } else {
              const oldIdx = oldList.indexOf(id);
              if (oldIdx > 0 && newList[oldIdx - 1]) {
                newActive[p] = newList[oldIdx - 1]!;
              } else {
                newActive[p] = newList[newList.length - 1]!;
              }
            }
          } else if (newActive[p] != null && !newList.includes(newActive[p]!)) {
            // Belt-and-braces: active id drifted out of the pane's list.
            newActive[p] = newList[newList.length - 1] ?? null;
          }
        }
        const newTabs = { ...tabs };
        delete newTabs[id];
        set({
          tabs: newTabs,
          panes: newPanes,
          activeByPane: newActive,
          hasAnyTab: newPanes.top.length > 0 || newPanes.bottom.length > 0,
        });
      },

      activateTab: (pane, id) =>
        set((s) => ({ activeByPane: { ...s.activeByPane, [pane]: id } })),

      moveTab: (id, toPane, index) => {
        const { panes, activeByPane } = get();
        const currentPane: PaneId | null =
          (["top", "bottom"] as PaneId[]).find((p) => panes[p].includes(id)) ?? null;
        if (!currentPane) return;
        const newPanes: Record<PaneId, string[]> = {
          top: [...panes.top],
          bottom: [...panes.bottom],
        };
        newPanes[currentPane] = newPanes[currentPane].filter((t) => t !== id);
        const targetList = newPanes[toPane];
        const at =
          index == null || index < 0 || index > targetList.length
            ? targetList.length
            : index;
        targetList.splice(at, 0, id);
        set({
          panes: newPanes,
          activeByPane: { ...activeByPane, [toPane]: id },
          hasAnyTab: newPanes.top.length > 0 || newPanes.bottom.length > 0,
        });
      },

      pruneSessions: (live) => {
        const { tabs, closeTab } = get();
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
    }),
    {
      name: STORAGE_KEY,
      storage: createJSONStorage(() => localStorage),
      // Only persist raw state; `hasAnyTab` is derived and the actions
      // are functions.
      partialize: (state) =>
        ({
          tabs: state.tabs,
          panes: state.panes,
          activeByPane: state.activeByPane,
        }) as PersistedShape,
      // Self-heal invariant violations that older builds may have
      // persisted: filter pane lists to ids that exist in `tabs`, and
      // pick a sensible active id when the persisted one is missing.
      merge: (persisted, current) => {
        const p = (persisted ?? {}) as Partial<PersistedShape>;
        const hTabs = p.tabs ?? {};
        const hPanes: Record<PaneId, string[]> = {
          top: (p.panes?.top ?? []).filter((id) => id in hTabs),
          bottom: (p.panes?.bottom ?? []).filter((id) => id in hTabs),
        };
        const pickActive = (pane: PaneId): string | null => {
          const ids = hPanes[pane];
          if (ids.length === 0) return null;
          const persistedId = p.activeByPane?.[pane];
          if (persistedId && ids.includes(persistedId)) return persistedId;
          return ids[ids.length - 1]!;
        };
        return {
          ...current,
          tabs: hTabs,
          panes: hPanes,
          activeByPane: { top: pickActive("top"), bottom: pickActive("bottom") },
          hasAnyTab: hPanes.top.length > 0 || hPanes.bottom.length > 0,
        };
      },
    },
  ),
);

/** Back-compat hook matching the pre-zustand API. Subscribes to the
 * whole state, so every mutation triggers a re-render — same behaviour
 * as the old context provider. New consumers that care about re-render
 * cost should call `useTabStore(selector)` directly. */
export function useTabs(): TabStore {
  return useTabStore();
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
  return kind === "timeline" ? "bottom" : "top";
}

function basename(p: string): string {
  const i = p.lastIndexOf("/");
  return i === -1 ? p : p.slice(i + 1);
}
