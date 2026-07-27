import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { StatsStrip } from "./StatsStrip";
import { appStatePayload, jsonResponse, statsPayload } from "../test/appState";
import type { NodeView } from "../api/types";

function installStatsFetch(
  payload: ReturnType<typeof statsPayload>,
  nodes: NodeView[] = [],
) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      jsonResponse(appStatePayload({ stats: payload, nodes })),
    ),
  );
}

function nodeView(overrides: Partial<NodeView> = {}): NodeView {
  return {
    id: "00000000-0000-0000-0000-000000000001",
    display_name: "sulion-enclave",
    protocol_version: 1,
    boot_id: "10000000-0000-0000-0000-000000000001",
    connection_state: "connected",
    connected_at: "2026-07-27T12:00:00Z",
    last_heartbeat_at: "2026-07-27T12:00:03Z",
    node_disconnected_at: null,
    heartbeat_timeout_seconds: 20,
    ...overrides,
  };
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

  it("surfaces development-node connectivity and heartbeat details", async () => {
    installStatsFetch(statsPayload(), [nodeView()]);
    render(<StatsStrip />);
    await waitFor(() => {
      expect(screen.getByText("connected")).toBeDefined();
    });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await user.click(screen.getByLabelText(/toggle stats details/i));
    expect(screen.getByText("Development node")).toBeDefined();
    expect(screen.getByText("sulion-enclave")).toBeDefined();
    expect(screen.getByText("v1")).toBeDefined();
  });

  it("surfaces a disconnected node without inventing compatibility state", async () => {
    installStatsFetch(statsPayload(), [
      nodeView({
        connection_state: "disconnected",
        last_heartbeat_at: null,
      }),
    ]);
    render(<StatsStrip />);
    await waitFor(() => {
      expect(screen.getByText("disconnected")).toBeDefined();
    });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await user.click(screen.getByLabelText(/toggle stats details/i));
    expect(screen.getByText("never")).toBeDefined();
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
