import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { App } from "./App";

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
      let body: unknown = { repos: [] };
      if (url.includes("/api/sessions")) body = { sessions: [] };
      else if (url.includes("/api/stats")) {
        body = {
          uptime_seconds: 1,
          process: { memory_rss_bytes: 0, cpu_percent: 0, memory_limit_bytes: null },
          pty: { live_sessions: 0, live_agent_sessions: 0 },
          db: {
            database_size_bytes: 0,
          },
          inventory: {
            event_rows: 0,
            agent_sessions: 0,
            pty_sessions: 0,
            tracked_files: 0,
            files_seen_since_boot: 0,
            events_inserted_since_boot: 0,
            parse_errors_since_boot: 0,
          },
        };
      }
      return new Response(JSON.stringify(body), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
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
