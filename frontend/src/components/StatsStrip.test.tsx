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
    ingester: {
      files_seen_total: 100,
      events_inserted_total: 250,
      parse_errors_total: 0,
    },
    pty: { tracked_sessions: 3 },
    db: {
      database_size_bytes: 50 * 1024 * 1024,
      events_rowcount: 1234,
      agent_sessions_rowcount: 7,
      pty_sessions_rowcount: 5,
      ingester_state_rowcount: 7,
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

  it("renders the compact mem/cpu/sessions summary after the first poll", async () => {
    installStatsFetch(statsPayload());
    render(<StatsStrip />);
    await waitFor(() => {
      // "120 / 500 MB"
      expect(screen.getByText(/120 \/ 500 MB/i)).toBeDefined();
      expect(screen.getByText(/4%/)).toBeDefined();
      expect(screen.getByText(/▶ 3/)).toBeDefined();
    });
  });

  it("expands to show db size and ingester totals", async () => {
    installStatsFetch(statsPayload());
    render(<StatsStrip />);
    await waitFor(() => {
      expect(screen.getByText(/120 \/ 500 MB/i)).toBeDefined();
    });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await user.click(screen.getByLabelText(/toggle stats details/i));
    // One of the <dt> entries is "events"; there are other text matches
    // but the exact dt text is unique.
    expect(screen.getByText((t) => t === "events")).toBeDefined();
    expect(screen.getByText("1,234")).toBeDefined();
    expect(screen.getByText(/50.0 MB/)).toBeDefined();
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
