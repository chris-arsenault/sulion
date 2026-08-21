// Instance stability: tab content must survive pane moves, display-mode
// switches, and peek adoption without unmounting. The terminal stub
// counts mounts/unmounts; a remount would re-attach the WS in the real
// component, which is exactly what the registry exists to prevent.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useEffect } from "react";

const terminalLife = vi.hoisted(() => ({ mounts: 0, unmounts: 0 }));

vi.mock("../LibraryPanel", () => ({
  LibraryPanel: () => null,
}));

vi.mock("../StatsStrip", () => ({
  StatsStrip: () => null,
}));

vi.mock("../TerminalPane", () => ({
  TerminalPane: ({ sessionId }: { sessionId: string }) => {
    useEffect(() => {
      terminalLife.mounts += 1;
      return () => {
        terminalLife.unmounts += 1;
      };
    }, []);
    return <div data-testid="stub-terminal">{sessionId}</div>;
  },
}));

vi.mock("../TimelinePane", () => ({
  TimelinePane: ({ sessionId }: { sessionId?: string }) => (
    <div data-testid="stub-timeline">{sessionId}</div>
  ),
}));

vi.mock("../ui", async () => {
  const actual = await vi.importActual<typeof import("../ui")>("../ui");
  return {
    ...actual,
    Tooltip: ({ children }: { children: unknown }) => children,
  };
});

import { ContextMenuHost } from "../common/ContextMenu";
import { Layout } from "../Layout";
import { resetDisplayStore, useDisplay } from "../../state/DisplayStore";
import { resetTabStore, useTabStore } from "../../state/TabStore";
import { resetTabHostRegistry } from "./registry";
import { appStatePayload, jsonResponse } from "../../test/appState";

const SESSION_ID = "abc12345-0000-0000-0000-000000000000";
const STUB_TERMINAL = "stub-terminal";
const STUB_TERMINAL_SELECTOR = '[data-testid="stub-terminal"]';

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

function openSessionTabs() {
  act(() => {
    useTabStore.getState().openTab({ kind: "terminal", sessionId: SESSION_ID }, "top");
    useTabStore.getState().openTab({ kind: "timeline", sessionId: SESSION_ID }, "bottom");
  });
}

function terminalTabId(): string {
  const state = useTabStore.getState();
  const id = Object.keys(state.tabs).find(
    (candidate) => state.tabs[candidate]?.kind === "terminal",
  );
  if (!id) throw new Error("terminal tab missing");
  return id;
}

beforeEach(() => {
  window.localStorage.clear();
  terminalLife.mounts = 0;
  terminalLife.unmounts = 0;
  resetTabStore();
  resetDisplayStore();
  resetTabHostRegistry();
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

describe("TabHost instance stability", () => {
  it("keeps the terminal mounted when its tab moves between panes", async () => {
    openSessionTabs();
    renderLayout();
    await screen.findByTestId(STUB_TERMINAL);
    const node = screen.getByTestId(STUB_TERMINAL);
    expect(terminalLife.mounts).toBe(1);

    act(() => useTabStore.getState().moveTab(terminalTabId(), "bottom"));
    await waitFor(() => expect(screen.getByTestId(STUB_TERMINAL)).toBeDefined());
    expect(screen.getByTestId(STUB_TERMINAL)).toBe(node);
    expect(terminalLife.unmounts).toBe(0);

    act(() => useTabStore.getState().moveTab(terminalTabId(), "top"));
    expect(screen.getByTestId(STUB_TERMINAL)).toBe(node);
    expect(terminalLife.unmounts).toBe(0);
  });

  it("keeps the terminal alive (detached) through display-mode switches", async () => {
    openSessionTabs();
    renderLayout();
    await screen.findByTestId(STUB_TERMINAL);
    const node = screen.getByTestId(STUB_TERMINAL);

    act(() => useDisplay.getState().setMode("timeline"));
    // Hidden from the document, but never unmounted.
    await waitFor(() => expect(screen.queryByTestId(STUB_TERMINAL)).toBeNull());
    expect(terminalLife.unmounts).toBe(0);

    act(() => useDisplay.getState().setMode("split"));
    await waitFor(() => expect(screen.getByTestId(STUB_TERMINAL)).toBeDefined());
    expect(screen.getByTestId(STUB_TERMINAL)).toBe(node);
    expect(terminalLife.mounts).toBe(1);
  });

  it("the peek overlay adopts the same terminal instance and returns it", async () => {
    openSessionTabs();
    renderLayout();
    await screen.findByTestId(STUB_TERMINAL);
    const node = screen.getByTestId(STUB_TERMINAL);
    const user = userEvent.setup();

    act(() => useDisplay.getState().setMode("timeline"));
    await user.keyboard("{Control>}{Shift>}E{/Shift}{/Control}");
    const dialog = await screen.findByRole("dialog");
    await waitFor(() =>
      expect(dialog.querySelector(STUB_TERMINAL_SELECTOR)).toBe(node),
    );
    expect(terminalLife.mounts).toBe(1);
    expect(terminalLife.unmounts).toBe(0);

    await user.keyboard("{Control>}{Shift>}E{/Shift}{/Control}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    act(() => useDisplay.getState().setMode("split"));
    await waitFor(() => expect(screen.getByTestId(STUB_TERMINAL)).toBe(node));
    expect(terminalLife.mounts).toBe(1);
    expect(terminalLife.unmounts).toBe(0);
  });

  it("peeking auto-opens a missing counterpart tab", async () => {
    act(() => {
      useTabStore
        .getState()
        .openTab({ kind: "timeline", sessionId: SESSION_ID }, "bottom");
    });
    renderLayout();
    const user = userEvent.setup();

    act(() => useDisplay.getState().setMode("timeline"));
    await user.keyboard("{Control>}{Shift>}E{/Shift}{/Control}");
    const dialog = await screen.findByRole("dialog");
    await waitFor(() =>
      expect(dialog.querySelector(STUB_TERMINAL_SELECTOR)).not.toBeNull(),
    );
    // The counterpart terminal tab now exists in the store.
    expect(
      Object.values(useTabStore.getState().tabs).some(
        (tab) => tab.kind === "terminal" && tab.sessionId === SESSION_ID,
      ),
    ).toBe(true);
  });

  it("closing the tab unmounts its content", async () => {
    openSessionTabs();
    renderLayout();
    await screen.findByTestId(STUB_TERMINAL);

    act(() => useTabStore.getState().closeTab(terminalTabId()));
    await waitFor(() => expect(terminalLife.unmounts).toBe(1));
    expect(screen.queryByTestId(STUB_TERMINAL)).toBeNull();
  });
});
