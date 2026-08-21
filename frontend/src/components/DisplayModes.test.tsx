// Display modes: single-pane work area, layout hotkeys, the peek
// overlay, and the display settings view of the monitor overlay.
// TerminalPane is stubbed — xterm needs canvas APIs jsdom lacks — so
// these tests exercise the mode plumbing, not terminal rendering.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("./LibraryPanel", () => ({
  LibraryPanel: () => null,
}));

vi.mock("./StatsStrip", () => ({
  StatsStrip: () => null,
}));

vi.mock("./TerminalPane", () => ({
  TerminalPane: ({ sessionId }: { sessionId: string }) => (
    <div data-testid="stub-terminal">{sessionId}</div>
  ),
}));

vi.mock("./TimelinePane", () => ({
  TimelinePane: ({ sessionId }: { sessionId?: string }) => (
    <div data-testid="stub-timeline">{sessionId}</div>
  ),
}));

vi.mock("./ui", async () => {
  const actual = await vi.importActual<typeof import("./ui")>("./ui");
  return {
    ...actual,
    Tooltip: ({ children }: { children: unknown }) => children,
  };
});

import { ContextMenuHost } from "./common/ContextMenu";
import { Layout } from "./Layout";
import { resetDisplayStore, useDisplay } from "../state/DisplayStore";
import { resetTabStore, useTabStore } from "../state/TabStore";
import { appStatePayload, jsonResponse } from "../test/appState";

function stubMatchMedia(matches: (q: string) => boolean) {
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches: matches(query),
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
}

function renderLayout() {
  stubMatchMedia(() => false); // desktop
  return render(
    <>
      <Layout />
      <ContextMenuHost />
    </>,
  );
}

const SESSION_ID = "abc12345-0000-0000-0000-000000000000";

// Minimal live SessionView so TerminalOrEndedPane mounts the (stubbed)
// terminal instead of the orphan placeholder.
const SESSION_FIXTURE = {
  id: SESSION_ID,
  repo: "alpha",
  working_dir: "/tmp/alpha",
  state: "live",
  created_at: "2026-05-02T00:00:00Z",
  ended_at: null,
  exit_code: null,
  current_session_uuid: null,
  current_session_agent: null,
  last_event_at: null,
  timeline_revision: 0,
  label: null,
  pinned: false,
  color: null,
  future_prompts_pending_count: 0,
  agent_runtime: {
    agent: null,
    state: "none",
    started_at: null,
    ended_at: null,
    exit_code: null,
  },
  agent_metadata: null,
};

function openSessionTabs(sessionId: string) {
  act(() => {
    useTabStore.getState().openTab({ kind: "terminal", sessionId }, "top");
    useTabStore.getState().openTab({ kind: "timeline", sessionId }, "bottom");
  });
}

beforeEach(() => {
  window.localStorage.clear();
  resetTabStore();
  resetDisplayStore();
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo) => {
      const url = new URL(
        typeof input === "string" ? input : (input as Request).url,
        "http://localhost",
      );
      if (url.pathname === "/api/app-state") {
        return jsonResponse(
          appStatePayload({ sessions: [SESSION_FIXTURE], repos: [] }),
        );
      }
      if (url.pathname.startsWith("/api/library/")) {
        return jsonResponse([]);
      }
      return new Response("", { status: 404 });
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("display modes", () => {
  it("Ctrl+Shift+D cycles the display mode", async () => {
    renderLayout();
    const user = userEvent.setup();
    expect(useDisplay.getState().mode).toBe("split");
    await user.keyboard("{Control>}{Shift>}D{/Shift}{/Control}");
    expect(useDisplay.getState().mode).toBe("terminal");
    await user.keyboard("{Control>}{Shift>}D{/Shift}{/Control}");
    expect(useDisplay.getState().mode).toBe("timeline");
    await user.keyboard("{Control>}{Shift>}D{/Shift}{/Control}");
    expect(useDisplay.getState().mode).toBe("split");
  });

  it("Ctrl+Shift+B collapses and restores the sidebar", async () => {
    renderLayout();
    const user = userEvent.setup();
    expect(screen.getByLabelText("Workspace")).toBeDefined();
    await user.keyboard("{Control>}{Shift>}B{/Shift}{/Control}");
    await waitFor(() => expect(screen.queryByLabelText("Workspace")).toBeNull());
    await user.keyboard("{Control>}{Shift>}B{/Shift}{/Control}");
    await waitFor(() => expect(screen.getByLabelText("Workspace")).toBeDefined());
  });

  it("terminal mode renders one pane and hides timeline tabs", async () => {
    openSessionTabs(SESSION_ID);
    renderLayout();
    expect(document.querySelector(".wa--split")).not.toBeNull();
    expect(document.querySelector(".wa__tab-kind--timeline")).not.toBeNull();

    act(() => useDisplay.getState().setMode("terminal"));
    await waitFor(() => {
      expect(document.querySelector(".wa--single")).not.toBeNull();
    });
    expect(document.querySelector(".wa--split")).toBeNull();
    expect(document.querySelector(".wa__tab-kind--timeline")).toBeNull();
    expect(document.querySelector(".wa__tab-kind--terminal")).not.toBeNull();
    expect(screen.getByTestId("stub-terminal")).toBeDefined();
  });

  it("timeline mode hides terminal tabs and activates the session's timeline", async () => {
    openSessionTabs(SESSION_ID);
    act(() => {
      // Leave the terminal tab active so the mode switch has to fall
      // back to the same session's timeline tab.
      const terminalId = useTabStore
        .getState()
        .panes.top.find((id) => useTabStore.getState().tabs[id]?.kind === "terminal");
      if (terminalId) useTabStore.getState().activateTab("top", terminalId);
    });
    renderLayout();

    act(() => useDisplay.getState().setMode("timeline"));
    await waitFor(() => {
      expect(document.querySelector(".wa--single")).not.toBeNull();
    });
    expect(document.querySelector(".wa__tab-kind--terminal")).toBeNull();
    expect(screen.getByTestId("stub-timeline")).toBeDefined();
  });

  it("Ctrl+Shift+E peeks at the hidden projection and is a no-op in split", async () => {
    openSessionTabs(SESSION_ID);
    renderLayout();
    const user = userEvent.setup();

    await user.keyboard("{Control>}{Shift>}E{/Shift}{/Control}");
    expect(screen.queryByRole("dialog")).toBeNull();

    act(() => useDisplay.getState().setMode("timeline"));
    await user.keyboard("{Control>}{Shift>}E{/Shift}{/Control}");
    const dialog = await screen.findByRole("dialog");
    // Timeline mode hides the terminal, so the peek shows it.
    expect(dialog.querySelector('[data-testid="stub-terminal"]')).not.toBeNull();

    await user.keyboard("{Control>}{Shift>}E{/Shift}{/Control}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("the monitor overlay gains a display settings view that sets the mode", async () => {
    renderLayout();
    const user = userEvent.setup();
    await user.keyboard("{Control>}m{/Control}");
    expect(await screen.findByTestId("monitor-pane")).toBeDefined();

    await user.click(screen.getByRole("button", { name: "display" }));
    expect(await screen.findByTestId("display-settings")).toBeDefined();

    await user.click(screen.getByRole("radio", { name: /terminal only/i }));
    expect(useDisplay.getState().mode).toBe("terminal");

    // The other overlay views remain reachable from the settings view.
    await user.click(screen.getByRole("button", { name: "metrics" }));
    expect(await screen.findByTestId("metrics-pane")).toBeDefined();
  });
});
