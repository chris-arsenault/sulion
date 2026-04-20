import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { StatsStrip } from "./StatsStrip";

function statsPayload(overrides: Record<string, unknown> = {}) {
  return {
    uptime_seconds: 3_700, // ~1h 1m
    process: {
      memory_rss_bytes: 120 * 1024 * 1024,
      cpu_percent: 4.2,
      memory_limit_bytes: 500 * 1024 * 1024,
    },
    pty: { live_sessions: 3, live_agent_sessions: 2 },
    db: {
      database_size_bytes: 50 * 1024 * 1024,
    },
    inventory: {
      event_rows: 1234,
      agent_sessions: 7,
      pty_sessions: 5,
      tracked_files: 7,
      files_seen_since_boot: 100,
      events_inserted_since_boot: 250,
      parse_errors_since_boot: 0,
    },
    ...overrides,
  };
}

function installStatsFetch(payload: unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      new Response(JSON.stringify(payload), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    ),
  );
}

describe("StatsStrip", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("renders the compact live-load summary after the first poll", async () => {
    installStatsFetch(statsPayload());
    render(<StatsStrip />);
    await waitFor(() => {
      expect(screen.getByText(/120 \/ 500 MB/i)).toBeDefined();
      expect(screen.getByText(/4%/)).toBeDefined();
      expect(screen.getByText(/50.0 MB/)).toBeDefined();
      expect(screen.getByText(/^3$/)).toBeDefined();
    });
  });

  it("expands to separate current load from inventory", async () => {
    installStatsFetch(statsPayload());
    render(<StatsStrip />);
    await waitFor(() => {
      expect(screen.getByText(/120 \/ 500 MB/i)).toBeDefined();
    });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await user.click(screen.getByLabelText(/toggle stats details/i));
    expect(screen.getByText("Current")).toBeDefined();
    expect(screen.getByText("Inventory")).toBeDefined();
    expect(screen.getByText((t) => t === "event rows")).toBeDefined();
    expect(screen.getByText("1,234")).toBeDefined();
    expect(screen.getAllByText(/50.0 MB/)).toHaveLength(2);
    expect(screen.getByText((t) => t === "agent PTYs")).toBeDefined();
  });

  it("shows 'stats unavailable' when the endpoint fails on first load", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("", { status: 500 })),
    );
    render(<StatsStrip />);
    await waitFor(() => {
      expect(screen.getByText(/stats unavailable/i)).toBeDefined();
    });
  });
});
