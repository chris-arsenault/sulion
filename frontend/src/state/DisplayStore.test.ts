import { beforeEach, describe, expect, it } from "vitest";

import { resetDisplayStore, useDisplay } from "./DisplayStore";

describe("DisplayStore", () => {
  beforeEach(() => {
    window.localStorage.clear();
    resetDisplayStore();
  });

  it("defaults to split with the sidebar pinned", () => {
    expect(useDisplay.getState().mode).toBe("split");
    expect(useDisplay.getState().sidebarPinned).toBe(true);
  });

  it("cycles split → terminal → timeline → split", () => {
    const { cycleMode } = useDisplay.getState();
    cycleMode();
    expect(useDisplay.getState().mode).toBe("terminal");
    cycleMode();
    expect(useDisplay.getState().mode).toBe("timeline");
    cycleMode();
    expect(useDisplay.getState().mode).toBe("split");
  });

  it("persists the mode and rehydrates it", () => {
    useDisplay.getState().setMode("timeline");
    expect(window.localStorage.getItem("sulion.display.mode.v1")).toBe("timeline");
    useDisplay.setState({ mode: "split" });
    resetDisplayStore();
    expect(useDisplay.getState().mode).toBe("timeline");
  });

  it("persists the sidebar pin through the legacy key", () => {
    useDisplay.getState().toggleSidebar();
    expect(useDisplay.getState().sidebarPinned).toBe(false);
    expect(window.localStorage.getItem("sulion.sidebar.pinned.v1")).toBe("0");
    resetDisplayStore();
    expect(useDisplay.getState().sidebarPinned).toBe(false);
  });
});
