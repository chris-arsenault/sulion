import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  resetTabStore,
  unassociatedTerminalTimelineTabIds,
  useTabStore,
} from "./TabStore";

function openTerminal(sessionId: string, pane: "top" | "bottom" = "top") {
  return useTabStore.getState().openTab({ kind: "terminal", sessionId }, pane);
}

function openTimeline(sessionId: string, pane: "top" | "bottom" = "bottom") {
  return useTabStore.getState().openTab({ kind: "timeline", sessionId }, pane);
}

describe("TabStore pair-linked activation", () => {
  beforeEach(() => {
    resetTabStore();
  });

  afterEach(() => {
    resetTabStore();
  });

  it("activating a terminal also activates the paired timeline in the other pane", () => {
    const termA = openTerminal("session-a", "top");
    const timeA = openTimeline("session-a", "bottom");
    const termB = openTerminal("session-b", "top");
    const timeB = openTimeline("session-b", "bottom");

    // Start with B active on top, B on bottom.
    useTabStore.getState().activateTab("top", termB);
    expect(useTabStore.getState().activeByPane.top).toBe(termB);

    // Switching top to A should swing bottom to A's timeline.
    useTabStore.getState().activateTab("top", termA);
    expect(useTabStore.getState().activeByPane.top).toBe(termA);
    expect(useTabStore.getState().activeByPane.bottom).toBe(timeA);

    // And the reverse direction — clicking bottom timeline B swings top.
    useTabStore.getState().activateTab("bottom", timeB);
    expect(useTabStore.getState().activeByPane.bottom).toBe(timeB);
    expect(useTabStore.getState().activeByPane.top).toBe(termB);
  });

  it("sticky pane blocks paired auto-switches", () => {
    const termA = openTerminal("session-a", "top");
    const timeA = openTimeline("session-a", "bottom");
    openTerminal("session-b", "top");
    const timeB = openTimeline("session-b", "bottom");
    // openTab activates the newly opened tab in its pane, so the
    // seed state after these four opens is top=termB, bottom=timeB.
    expect(useTabStore.getState().activeByPane.bottom).toBe(timeB);

    // Lock the bottom pane: subsequent top clicks should not swing it.
    useTabStore.getState().setPaneSticky("bottom", true);
    useTabStore.getState().activateTab("top", termA);
    expect(useTabStore.getState().activeByPane.top).toBe(termA);
    expect(useTabStore.getState().activeByPane.bottom).toBe(timeB);

    // Releasing the sticky flag restores the pairing; clicking the
    // same top tab again should now swing the bottom.
    useTabStore.getState().setPaneSticky("bottom", false);
    useTabStore.getState().activateTab("top", termA);
    expect(useTabStore.getState().activeByPane.bottom).toBe(timeA);
  });

  it("non-pairable kinds do not trigger paired switches", () => {
    openTerminal("session-a", "top");
    const timeA = openTimeline("session-a", "bottom");
    const fileTab = useTabStore
      .getState()
      .openTab({ kind: "file", repo: "alpha", path: "src/lib.rs" }, "top");

    useTabStore.getState().activateTab("top", fileTab);
    // Bottom should remain on its timeline; file tabs have no pair.
    expect(useTabStore.getState().activeByPane.top).toBe(fileTab);
    expect(useTabStore.getState().activeByPane.bottom).toBe(timeA);
  });
});

describe("TabStore clearTimelineFocus", () => {
  beforeEach(() => {
    resetTabStore();
  });

  afterEach(() => {
    resetTabStore();
  });

  it("strips focus fields from a timeline tab", () => {
    const id = useTabStore.getState().openTab(
      {
        kind: "timeline",
        sessionId: "session-a",
        focusTurnId: 42,
        focusPairId: "tool_xyz",
        focusKey: "k1",
      },
      "bottom",
    );

    useTabStore.getState().clearTimelineFocus(id);

    const tab = useTabStore.getState().tabs[id]!;
    expect(tab.focusTurnId).toBeUndefined();
    expect(tab.focusPairId).toBeUndefined();
    expect(tab.focusKey).toBeUndefined();
  });

  it("is a no-op for tabs with no focus set", () => {
    const id = useTabStore
      .getState()
      .openTab({ kind: "timeline", sessionId: "session-a" }, "bottom");
    const before = useTabStore.getState().tabs[id];
    useTabStore.getState().clearTimelineFocus(id);
    expect(useTabStore.getState().tabs[id]).toBe(before);
  });

  it("is a no-op for non-timeline tabs", () => {
    const id = useTabStore
      .getState()
      .openTab({ kind: "file", repo: "alpha", path: "src/lib.rs" }, "top");
    const before = useTabStore.getState().tabs[id];
    useTabStore.getState().clearTimelineFocus(id);
    expect(useTabStore.getState().tabs[id]).toBe(before);
  });
});

describe("unassociatedTerminalTimelineTabIds", () => {
  beforeEach(() => {
    resetTabStore();
  });

  afterEach(() => {
    resetTabStore();
  });

  it("returns terminal and session timeline tabs whose session is gone", () => {
    const liveTerm = openTerminal("session-live", "top");
    const liveTimeline = openTimeline("session-live", "bottom");
    const goneTerm = openTerminal("session-gone", "top");
    const goneTimeline = openTimeline("session-gone", "bottom");
    const repoTimeline = useTabStore
      .getState()
      .openTab({ kind: "timeline", repo: "alpha" }, "bottom");
    const fileTab = useTabStore
      .getState()
      .openTab({ kind: "file", repo: "alpha", path: "src/lib.rs" }, "top");

    const ids = unassociatedTerminalTimelineTabIds(
      useTabStore.getState().tabs,
      new Set(["session-live"]),
    );

    expect(ids).toEqual([goneTerm, goneTimeline]);
    expect(ids).not.toContain(liveTerm);
    expect(ids).not.toContain(liveTimeline);
    expect(ids).not.toContain(repoTimeline);
    expect(ids).not.toContain(fileTab);
  });
});

describe("TabStore rehydration drops retired tab kinds", () => {
  beforeEach(() => {
    resetTabStore();
  });

  afterEach(() => {
    resetTabStore();
    window.localStorage.clear();
  });

  it("skips a persisted tab whose kind no longer exists (e.g. the old plan tab)", async () => {
    // Seed localStorage exactly as the persist middleware would have,
    // including a retired "plan" tab that would crash the WorkArea
    // switches (React error #130) if rehydrated verbatim.
    window.localStorage.setItem(
      "sulion.tabs.v2",
      JSON.stringify({
        version: 0,
        state: {
          tabs: {
            term1: { id: "term1", kind: "terminal", sessionId: "s1" },
            plan1: { id: "plan1", kind: "plan", repo: "alpha" },
          },
          panes: { top: ["term1", "plan1"], bottom: [] },
          activeByPane: { top: "plan1", bottom: null },
          sticky: { top: false, bottom: false },
        },
      }),
    );

    await useTabStore.persist.rehydrate();

    const state = useTabStore.getState();
    expect(state.tabs.plan1).toBeUndefined();
    expect(state.tabs.term1?.kind).toBe("terminal");
    expect(state.panes.top).toEqual(["term1"]);
    // The retired tab was the persisted active one; fall back to a
    // surviving tab rather than dangling on a dropped id.
    expect(state.activeByPane.top).toBe("term1");
  });
});
