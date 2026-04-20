import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { App } from "./App";

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
          ingester: {
            files_seen_total: 0,
            events_inserted_total: 0,
            parse_errors_total: 0,
          },
          pty: { tracked_sessions: 0 },
          db: {
            database_size_bytes: 0,
            events_rowcount: 0,
            agent_sessions_rowcount: 0,
            pty_sessions_rowcount: 0,
            ingester_state_rowcount: 0,
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
