import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";

const STORAGE_KEY = "sulion.tabs.v2";

export type PaneId = "top" | "bottom";
export type TabKind =
  | "terminal"
  | "timeline"
  | "file"
  | "diff"
  | "ref";

/** Minimal registry entry. Kind plus the tab's stable handle is the
 * identity; everything else is the tab's own business. */
export interface TabData {
  id: string;
  kind: TabKind;
  /** For session-bound tabs (terminal, timeline). */
  sessionId?: string;
  /** For repo-bound tabs (file, diff). */
  repo?: string;
  /** File path for file/diff tabs; optional for diff (whole-repo). */
  path?: string;
  /** Library slug for reference tabs. */
  slug?: string;
  /** Timeline focus target. Ignored for other tab kinds. */
  focusTurnId?: number;
  /** Optional: a specific tool call within the focused turn. The
   * timeline pane expands that tool, collapses its siblings, and
   * gives it a persistent outline. Null-safe: falls back to turn-level
   * focus when absent. */
  focusPairId?: string;
  /** Changes on every focus request so repeated jumps still fire. */
  focusKey?: string;
}

export interface TabStore {
  tabs: Record<string, TabData>;
  panes: Record<PaneId, string[]>;
  activeByPane: Record<PaneId, string | null>;
  /** When a pane is sticky, activations in the other pane do not
   * propagate a paired session-swap into it. Toggled by the tab
   * context menu. */
  sticky: Record<PaneId, boolean>;
  hasAnyTab: boolean;
  openTab: (spec: Omit<TabData, "id">, pane?: PaneId) => string;
  closeTab: (id: string) => void;
  activateTab: (pane: PaneId, id: string) => void;
  moveTab: (id: string, toPane: PaneId, index?: number) => void;
  rebindSessionTabs: (fromSessionId: string, toSessionId: string) => void;
  setPaneSticky: (pane: PaneId, value: boolean) => void;
  /** Strip `focusTurnId` / `focusPairId` / `focusKey` from a timeline
   * tab. Called when the user manually picks a turn, so subsequent
   * polls (or tab revisits) don't snap the selection back to the old
   * focus target. */
  clearTimelineFocus: (id: string) => void;
}

interface PersistedTabs {
  tabs: Record<string, TabData>;
  panes: Record<PaneId, string[]>;
  activeByPane: Record<PaneId, string | null>;
  sticky: Record<PaneId, boolean>;
}

function initialState(): PersistedTabs & Pick<TabStore, "hasAnyTab"> {
  return {
    tabs: {},
    panes: { top: [], bottom: [] },
    activeByPane: { top: null, bottom: null },
    sticky: { top: false, bottom: false },
    hasAnyTab: false,
  };
}

function withDerived(
  state: PersistedTabs,
): PersistedTabs & Pick<TabStore, "hasAnyTab"> {
  return {
    ...state,
    hasAnyTab: state.panes.top.length > 0 || state.panes.bottom.length > 0,
  };
}

/** Tab kinds that pair-link across panes. Activating a terminal in
 * one pane should swing the other pane to the matching timeline for
 * that session, and vice versa. File/diff/ref tabs don't participate. */
function pairedKindOf(kind: TabKind | undefined): TabKind | null {
  if (kind === "terminal") return "timeline";
  if (kind === "timeline") return "terminal";
  return null;
}

export const useTabStore = create<TabStore>()(
  persist(
    (set, get) => ({
      ...initialState(),

      openTab: (spec, pane = defaultPaneFor(spec.kind)) => {
        const { tabs, panes } = get();
        const key = tabKey(spec);
        const existingId = Object.keys(tabs).find((id) => tabKey(tabs[id]!) === key);
        if (existingId) {
          const inPane =
            (["top", "bottom"] as PaneId[]).find((candidate) =>
              panes[candidate].includes(existingId),
            ) ?? null;
          if (inPane) {
            set((state) => ({
              tabs:
                spec.kind === "timeline" && spec.focusKey
                  ? {
                      ...state.tabs,
                      [existingId]: {
                        ...state.tabs[existingId],
                        focusTurnId: spec.focusTurnId,
                        focusPairId: spec.focusPairId,
                        focusKey: spec.focusKey,
                      } as TabData,
                    }
                  : state.tabs,
              activeByPane: { ...state.activeByPane, [inPane]: existingId },
            }));
          }
          return existingId;
        }

        const id = crypto.randomUUID();
        const data: TabData = { ...spec, id };
        set((state) =>
          withDerived({
            tabs: { ...state.tabs, [id]: data },
            panes: { ...state.panes, [pane]: [...state.panes[pane], id] },
            activeByPane: { ...state.activeByPane, [pane]: id },
            sticky: state.sticky,
          }),
        );
        return id;
      },

      closeTab: (id) => {
        const { panes, activeByPane, tabs, sticky } = get();
        set(
          withDerived(removeTabFromState({
            tabs,
            panes,
            activeByPane,
            sticky,
          }, id)),
        );
      },

      activateTab: (pane, id) =>
        set((state) => {
          const nextActive = { ...state.activeByPane, [pane]: id };
          // Pair-link: activating a terminal (or timeline) for a
          // session in one pane should swing the other pane to the
          // same session's paired view — unless the other pane is
          // sticky, in which case it's explicitly pinned.
          const activated = state.tabs[id];
          const other: PaneId = pane === "top" ? "bottom" : "top";
          const paired = pairedKindOf(activated?.kind);
          if (
            activated?.sessionId &&
            paired &&
            !state.sticky[other]
          ) {
            const pairedId = state.panes[other].find((candidateId) => {
              const candidate = state.tabs[candidateId];
              return (
                candidate?.kind === paired &&
                candidate.sessionId === activated.sessionId
              );
            });
            if (pairedId && pairedId !== state.activeByPane[other]) {
              nextActive[other] = pairedId;
            }
          }
          return { activeByPane: nextActive };
        }),

      moveTab: (id, toPane, index) => {
        const { panes, activeByPane, tabs, sticky } = get();
        const currentPane =
          (["top", "bottom"] as PaneId[]).find((candidate) =>
            panes[candidate].includes(id),
          ) ?? null;
        if (!currentPane) return;

        const nextPanes: Record<PaneId, string[]> = {
          top: [...panes.top],
          bottom: [...panes.bottom],
        };
        nextPanes[currentPane] = nextPanes[currentPane].filter((tabId) => tabId !== id);

        const targetList = nextPanes[toPane];
        const at =
          index == null || index < 0 || index > targetList.length
            ? targetList.length
            : index;
        targetList.splice(at, 0, id);

        set(
          withDerived({
            tabs,
            panes: nextPanes,
            activeByPane: { ...activeByPane, [toPane]: id },
            sticky,
          }),
        );
      },

      setPaneSticky: (pane, value) =>
        set((state) => ({
          sticky: { ...state.sticky, [pane]: value },
        })),

      clearTimelineFocus: (id) =>
        set((state) => {
          const tab = state.tabs[id];
          if (
            !tab
            || tab.kind !== "timeline"
            || (tab.focusTurnId == null
              && tab.focusPairId == null
              && tab.focusKey == null)
          ) {
            return state;
          }
          const stripped: TabData = { ...tab };
          delete stripped.focusTurnId;
          delete stripped.focusPairId;
          delete stripped.focusKey;
          return {
            tabs: { ...state.tabs, [id]: stripped },
          };
        }),

      rebindSessionTabs: (fromSessionId, toSessionId) => {
        if (fromSessionId === toSessionId) return;
        const { tabs, panes, activeByPane, sticky } = get();
        let nextState: PersistedTabs = {
          tabs: { ...tabs },
          panes: { top: [...panes.top], bottom: [...panes.bottom] },
          activeByPane: { ...activeByPane },
          sticky,
        };
        let changed = false;

        for (const id of Object.keys(nextState.tabs)) {
          const tab = nextState.tabs[id];
          if (!tab || tab.sessionId !== fromSessionId) continue;

          const rebound: TabData = { ...tab, sessionId: toSessionId };
          const duplicateId = Object.keys(nextState.tabs).find(
            (candidateId) =>
              candidateId !== id &&
              tabKey(nextState.tabs[candidateId]!) === tabKey(rebound),
          );

          if (duplicateId) {
            nextState = removeTabFromState(nextState, id);
          } else {
            nextState.tabs[id] = rebound;
          }
          changed = true;
        }

        if (!changed) return;
        set(withDerived(nextState));
      },
    }),
    {
      name: STORAGE_KEY,
      storage: createJSONStorage(() => localStorage),
      partialize: (state): PersistedTabs => ({
        tabs: state.tabs,
        panes: state.panes,
        activeByPane: state.activeByPane,
        sticky: state.sticky,
      }),
      merge: (persisted, current) => {
        const envelope = (persisted ?? {}) as Partial<PersistedTabs> & {
          state?: Partial<PersistedTabs>;
        };
        const parsed =
          envelope && typeof envelope === "object" && "state" in envelope
            ? (envelope.state ?? {})
            : (envelope as Partial<PersistedTabs>);
        const hydratedTabs: Record<string, TabData> = {};
        for (const [id, tab] of Object.entries(parsed.tabs ?? {}) as Array<
          [string, Partial<TabData>]
        >) {
          if (!tab || typeof tab !== "object") continue;
          hydratedTabs[id] = {
            id: tab.id ?? id,
            kind: tab.kind,
            sessionId: tab.sessionId,
            repo: tab.repo,
            path: tab.path,
            slug: tab.slug,
            focusTurnId: tab.focusTurnId,
            focusPairId: tab.focusPairId,
            focusKey: tab.focusKey,
          } as TabData;
        }

        const hydratedPanes: Record<PaneId, string[]> = {
          top: (parsed.panes?.top ?? []).filter((id) => id in hydratedTabs),
          bottom: (parsed.panes?.bottom ?? []).filter((id) => id in hydratedTabs),
        };
        const pickActive = (pane: PaneId): string | null => {
          const ids = hydratedPanes[pane];
          if (ids.length === 0) return null;
          const persistedId = parsed.activeByPane?.[pane];
          if (persistedId && ids.includes(persistedId)) return persistedId;
          return ids[ids.length - 1]!;
        };

        const hydratedSticky: Record<PaneId, boolean> = {
          top: Boolean(parsed.sticky?.top),
          bottom: Boolean(parsed.sticky?.bottom),
        };

        return {
          ...current,
          ...withDerived({
            tabs: hydratedTabs,
            panes: hydratedPanes,
            activeByPane: {
              top: pickActive("top"),
              bottom: pickActive("bottom"),
            },
            sticky: hydratedSticky,
          }),
        };
      },
    },
  ),
);

export function useTabs<T>(selector: (state: TabStore) => T): T {
  return useTabStore(selector);
}

export function resetTabStore() {
  if (typeof window !== "undefined") {
    try {
      window.localStorage.removeItem(STORAGE_KEY);
    } catch {
      // storage unavailable
    }
  }
  useTabStore.setState(initialState());
}

function removeTabFromState(state: PersistedTabs, id: string): PersistedTabs {
  const nextPanes: Record<PaneId, string[]> = {
    top: state.panes.top.filter((tabId) => tabId !== id),
    bottom: state.panes.bottom.filter((tabId) => tabId !== id),
  };
  const nextActive: Record<PaneId, string | null> = { ...state.activeByPane };

  for (const pane of ["top", "bottom"] as PaneId[]) {
    const oldList = state.panes[pane];
    const newList = nextPanes[pane];
    if (nextActive[pane] === id) {
      if (newList.length === 0) {
        nextActive[pane] = null;
      } else {
        const oldIdx = oldList.indexOf(id);
        nextActive[pane] =
          oldIdx > 0 && newList[oldIdx - 1]
            ? newList[oldIdx - 1]!
            : newList[newList.length - 1]!;
      }
    } else if (nextActive[pane] != null && !newList.includes(nextActive[pane]!)) {
      nextActive[pane] = newList[newList.length - 1] ?? null;
    }
  }

  const nextTabs = { ...state.tabs };
  delete nextTabs[id];
  return {
    tabs: nextTabs,
    panes: nextPanes,
    activeByPane: nextActive,
    sticky: state.sticky,
  };
}

/** Canonical key that de-duplicates a tab spec. */
export function tabKey(
  spec: Pick<TabData, "kind" | "sessionId" | "repo" | "path" | "slug">,
): string {
  switch (spec.kind) {
    case "terminal":
      return `${spec.kind}:${spec.sessionId ?? ""}`;
    case "timeline":
      return spec.repo
        ? `timeline:repo:${spec.repo}`
        : `timeline:session:${spec.sessionId ?? ""}`;
    case "file":
      return `file:${spec.repo ?? ""}:${spec.path ?? ""}`;
    case "diff":
      return `diff:${spec.repo ?? ""}:${spec.path ?? ""}`;
    case "ref":
      return `ref:${spec.slug ?? ""}`;
  }
}

function defaultPaneFor(kind: TabKind): PaneId {
  return kind === "timeline" ? "bottom" : "top";
}
