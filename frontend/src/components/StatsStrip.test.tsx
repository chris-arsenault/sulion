import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { StatsStrip } from "./StatsStrip";
import { appStatePayload, jsonResponse, statsPayload } from "../test/appState";

function installStatsFetch(payload: ReturnType<typeof statsPayload>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      jsonResponse(appStatePayload({ stats: payload })),
    ),
  );
}

describe("StatsStrip", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.spyOn(console, "error").mockImplementation(() => {});
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
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
