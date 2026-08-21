// Which tabs are actually presented somewhere on screen. Shared by the
// work area (what to render in strips/slots) and TabHost (the `active`
// prop that gates polling), so the two can never disagree.

import type { DisplayMode } from "../../state/DisplayStore";
import { useDisplay } from "../../state/DisplayStore";
import type { Maybe } from "../../lib/types";
import type { PaneId, TabData } from "../../state/TabStore";
import { useTabs } from "../../state/TabStore";
import { useSessions } from "../../state/SessionStore";

export interface TabLayoutState {
  tabs: Record<string, TabData>;
  panes: Record<PaneId, string[]>;
  activeByPane: Record<PaneId, string | null>;
}

/** Whether a tab renders in the given display mode. Terminal-only hides
 * timeline tabs and vice versa; every other tab kind always shows. */
export function tabVisibleInMode(tab: Maybe<TabData>, mode: DisplayMode): boolean {
  if (!tab) return false;
  if (mode === "terminal") return tab.kind !== "timeline";
  if (mode === "timeline") return tab.kind !== "terminal";
  return true;
}

/** Active tab for the merged single pane: the stored active tab when it
 * is still visible, else the same session's counterpart tab matching the
 * mode, else the first visible tab. */
export function singlePaneActiveId(
  visibleIds: string[],
  storedActives: Array<string | null>,
  tabs: Record<string, TabData>,
  mode: DisplayMode,
): string | null {
  for (const candidate of storedActives) {
    if (candidate && visibleIds.includes(candidate)) return candidate;
  }
  const wantedKind = mode === "terminal" ? "terminal" : "timeline";
  for (const candidate of storedActives) {
    const hidden = candidate ? tabs[candidate] : undefined;
    if (!hidden?.sessionId) continue;
    const counterpart = visibleIds.find((id) => {
      const tab = tabs[id];
      return tab?.kind === wantedKind && tab.sessionId === hidden.sessionId;
    });
    if (counterpart) return counterpart;
  }
  return visibleIds[0] ?? null;
}

/** Tab ids currently presented on screen: the active tab of each shown
 * pane (both in split, the merged pane otherwise, one strip on mobile)
 * plus the peeked tab while the peek overlay is open. */
export function computeShownTabIds(
  state: TabLayoutState,
  mode: DisplayMode,
  isMobile: boolean,
  peekTabId: string | null,
): Set<string> {
  const shown = new Set<string>();
  const merged = [...state.panes.top, ...state.panes.bottom];

  if (isMobile) {
    const active =
      state.activeByPane.top ?? state.activeByPane.bottom ?? merged[0] ?? null;
    if (active) shown.add(active);
  } else if (mode === "split") {
    for (const paneId of ["top", "bottom"] as const) {
      const active = state.activeByPane[paneId];
      if (active) shown.add(active);
    }
  } else {
    const visibleIds = merged.filter((id) =>
      tabVisibleInMode(state.tabs[id], mode),
    );
    const active = singlePaneActiveId(
      visibleIds,
      [state.activeByPane.top, state.activeByPane.bottom],
      state.tabs,
      mode,
    );
    if (active) shown.add(active);
  }

  if (peekTabId) shown.add(peekTabId);
  return shown;
}

/** The tab the peek overlay presents: the open counterpart (terminal ↔
 * timeline) of the active tab's session, falling back to the sidebar's
 * selected session. Null when the peek is closed, the mode is split, or
 * no counterpart tab exists yet (Layout auto-opens one). */
export function usePeekTabId(): string | null {
  const mode = useDisplay((store) => store.mode);
  const peekOpen = useDisplay((store) => store.peekOpen);
  const selectedSessionId = useSessions((store) => store.selectedSessionId);
  return useTabs((store) => {
    if (!peekOpen || mode === "split") return null;
    const wantedKind = mode === "terminal" ? "timeline" : "terminal";
    const sessionId = peekSessionIdFrom(store, selectedSessionId);
    if (!sessionId) return null;
    return (
      Object.values(store.tabs).find(
        (tab) => tab.kind === wantedKind && tab.sessionId === sessionId,
      )?.id ?? null
    );
  });
}

/** Session the peek targets: the session of the active tab in either
 * pane (top pane wins), else the sidebar selection. */
export function peekSessionIdFrom(
  store: TabLayoutState,
  selectedSessionId: string | null,
): string | null {
  for (const paneId of ["top", "bottom"] as const) {
    const activeId = store.activeByPane[paneId];
    const tab = activeId ? store.tabs[activeId] : undefined;
    if (tab?.sessionId) return tab.sessionId;
  }
  return selectedSessionId;
}
