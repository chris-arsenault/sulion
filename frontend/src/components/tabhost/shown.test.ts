import { describe, expect, it } from "vitest";

import type { DisplayMode } from "../../state/DisplayStore";
import type { TabData } from "../../state/TabStore";
import {
  computeShownTabIds,
  singlePaneActiveId,
  type TabLayoutState,
} from "./shown";

const tabs: Record<string, TabData> = {
  terminal: {
    id: "terminal",
    kind: "terminal",
    sessionId: "session-a",
  },
  timeline: {
    id: "timeline",
    kind: "timeline",
    sessionId: "session-a",
  },
  file: {
    id: "file",
    kind: "file",
    repo: "alpha",
    path: "README.md",
  },
};

const layout: TabLayoutState = {
  tabs,
  panes: { top: ["terminal", "file"], bottom: ["timeline"] },
  activeByPane: { top: "terminal", bottom: "timeline" },
  activeSinglePaneId: "timeline",
};

describe("single-pane tab presentation", () => {
  it("prefers the explicit merged-pane selection", () => {
    expect(
      singlePaneActiveId(
        ["file", "timeline"],
        "file",
        ["terminal", "timeline"],
        tabs,
        "timeline",
      ),
    ).toBe("file");
  });

  it.each<DisplayMode>(["split", "terminal", "timeline"])(
    "forces the timeline projection on mobile when desktop mode is %s",
    (mode) => {
      expect([...computeShownTabIds(layout, mode, true, "terminal")]).toEqual([
        "timeline",
      ]);
    },
  );

  it("keeps both active panes in desktop split mode", () => {
    expect([...computeShownTabIds(layout, "split", false, null)]).toEqual([
      "terminal",
      "timeline",
    ]);
  });
});
