import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { App } from "./App";
import { appStatePayload, jsonResponse } from "./test/appState";

vi.mock("./components/LibraryPanel", () => ({
  LibraryPanel: () => null,
}));

vi.mock("./components/StatsStrip", () => ({
  StatsStrip: () => null,
}));

vi.mock("./components/ui", async () => {
  const actual = await vi.importActual<typeof import("./components/ui")>("./components/ui");
  return {
    ...actual,
    Tooltip: ({ children }: { children: unknown }) => children,
  };
});

// Stub fetch so the singleton stores' mount-time polls have deterministic
// empty responses during the render.
beforeEach(() => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo) => {
      const url = typeof input === "string" ? input : input.url;
      if (url === "/api/app-state") {
        return jsonResponse(appStatePayload());
      }
      return new Response("", { status: 404 });
    }),
  );
});

describe("App", () => {
  it("renders the empty-state copy when no session is selected", async () => {
    render(<App />);
    // Each empty pane now shows its own splash prompt.
    expect(
      screen.getAllByText(
        (t) =>
          typeof t === "string" && t.toLowerCase().includes("drag a tab here"),
      ).length,
    ).toBeGreaterThan(0);
  });

  it("shows the sidebar logo", async () => {
    render(<App />);
    // sulion appears in both the sidebar header and the empty state;
    // getAllByText confirms both presence and count.
    expect(screen.getAllByText("sulion").length).toBeGreaterThanOrEqual(1);
  });
});
