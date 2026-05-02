import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("./LibraryPanel", () => ({
  LibraryPanel: () => null,
}));

vi.mock("./StatsStrip", () => ({
  StatsStrip: () => null,
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
import { appCommands } from "../state/AppCommands";
import { appStatePayload, jsonResponse } from "../test/appState";

const mockState = {
  repos: [] as Array<Record<string, unknown>>,
  sessions: [] as Array<Record<string, unknown>>,
  trees: {} as Record<string, { path: string; entries: Array<Record<string, unknown>> }>,
  files: {} as Record<string, Record<string, unknown>>,
  diffs: {} as Record<string, string>,
};

function resetMockState() {
  mockState.repos = [];
  mockState.sessions = [];
  mockState.trees = {};
  mockState.files = {};
  mockState.diffs = {};
}

// matchMedia is not implemented in jsdom — stub it so useMediaQuery can
// observe breakpoints deterministically.
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

async function rightClick(
  el: HTMLElement,
  user: ReturnType<typeof userEvent.setup>,
) {
  await user.pointer({ keys: "[MouseRight]", target: el });
}

beforeEach(() => {
  resetMockState();
  vi.stubGlobal(
    "Worker",
    class MockWorker {
      onmessage: ((event: { data: { files: unknown[] } }) => void) | null = null;
      postMessage = vi.fn();
      terminate = vi.fn();
    } as unknown as typeof Worker,
  );
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo) => {
      const url = new URL(
        typeof input === "string" ? input : (input as Request).url,
        "http://localhost",
      );
      const json = jsonResponse;

      if (url.pathname === "/api/app-state") {
        return json(
          appStatePayload({
            sessions: mockState.sessions,
            repos: mockState.repos,
          }),
        );
      }

      if (url.pathname === "/api/library/references" || url.pathname === "/api/library/prompts") {
        return json([]);
      }

      const diffMatch = url.pathname.match(/^\/api\/repos\/([^/]+)\/git\/diff$/);
      if (diffMatch) {
        const repo = decodeURIComponent(diffMatch[1]!);
        const path = url.searchParams.get("path") ?? "";
        return json({ diff: mockState.diffs[`${repo}:${path}`] ?? "" });
      }

      const filesMatch = url.pathname.match(/^\/api\/repos\/([^/]+)\/files$/);
      if (filesMatch) {
        const repo = decodeURIComponent(filesMatch[1]!);
        const path = url.searchParams.get("path") ?? "";
        return json(mockState.trees[`${repo}:${path}`] ?? { path, entries: [] });
      }

      const fileMatch = url.pathname.match(/^\/api\/repos\/([^/]+)\/file$/);
      if (fileMatch) {
        const repo = decodeURIComponent(fileMatch[1]!);
        const path = url.searchParams.get("path") ?? "";
        return json(
          mockState.files[`${repo}:${path}`] ?? {
            path,
            mime: "text/plain",
            size: 0,
            binary: false,
            truncated: false,
            content: "",
          },
        );
      }

      const traceMatch = url.pathname.match(/^\/api\/repos\/([^/]+)\/file-trace$/);
      if (traceMatch) {
        const path = url.searchParams.get("path") ?? "";
        return json({
          path,
          dirty: null,
          current_diff: null,
          touches: [],
        });
      }

      return new Response("", { status: 404 });
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("Layout", () => {
  it("desktop (>=768px): renders the sidebar inline (no drawer)", async () => {
    stubMatchMedia(() => false); // no mobile media match
    render(
      <>
        <Layout />
        <ContextMenuHost />
      </>,
    );
    // Hamburger is mobile-only; it should not be in the DOM.
    expect(screen.queryByLabelText(/open sessions drawer/i)).toBeNull();
    // Each empty pane shows its own "drag a tab here" splash.
    expect(screen.getAllByText(/drag a tab here/i).length).toBeGreaterThan(0);
  });

  it("mobile (<768px): shows hamburger, hides sidebar until toggled", async () => {
    stubMatchMedia((q) => q.includes("max-width: 767px"));
    render(
      <>
        <Layout />
        <ContextMenuHost />
      </>,
    );
    const hamburger = screen.getByLabelText(/open sessions drawer/i);
    expect(hamburger).toBeDefined();

    // Drawer not open — the Sidebar-internal "No repos yet." text is
    // only visible once the drawer opens. (We can't rely solely on the
    // drawer aside being absent because the Sidebar component may still
    // render somewhere; instead we assert the aria-label "Sessions" is
    // not present on the drawer yet.)
    expect(screen.queryByLabelText(/^sessions$/i)).toBeNull();

    const user = userEvent.setup();
    await user.click(hamburger);
    await waitFor(() => {
      expect(screen.getByLabelText(/^sessions$/i)).toBeDefined();
    });
  });

  it("mobile empty state shows the phone-friendly hint", () => {
    stubMatchMedia((q) => q.includes("max-width: 767px"));
    render(
      <>
        <Layout />
        <ContextMenuHost />
      </>,
    );
    expect(screen.getByText(/open the drawer to pick a session/i)).toBeDefined();
  });

  it("opens file and diff tabs from sidebar tree actions", async () => {
    mockState.repos = [{ name: "alpha", path: "/tmp/alpha" }];
    mockState.trees["alpha:"] = {
      path: "",
      entries: [{ name: "app.ts", kind: "file", dirty: " M" }],
    };
    mockState.files["alpha:app.ts"] = {
      path: "app.ts",
      mime: "text/plain",
      size: 18,
      binary: false,
      truncated: false,
      content: "console.log('hi');",
    };
    mockState.diffs["alpha:app.ts"] = "diff --git a/app.ts b/app.ts";

    stubMatchMedia(() => false);
    render(
      <>
        <Layout />
        <ContextMenuHost />
      </>,
    );
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText("alpha")).toBeDefined());
    await user.click(screen.getByRole("button", { name: /Files/ }));
    const fileRow = await screen.findByText("app.ts");

    await rightClick(fileRow, user);
    await user.click(await screen.findByText("Open diff"));
    await waitFor(() => expect(screen.getByText("diff · app.ts")).toBeDefined());

    await user.click(fileRow);
    await waitFor(() => expect(screen.getAllByText("app.ts").length).toBeGreaterThan(1));
  });

  it("closes the mobile drawer when requested through app commands", async () => {
    stubMatchMedia((q) => q.includes("max-width: 767px"));
    render(
      <>
        <Layout />
        <ContextMenuHost />
      </>,
    );
    const user = userEvent.setup();

    await user.click(screen.getByLabelText(/open sessions drawer/i));
    await waitFor(() => expect(screen.getByLabelText(/^sessions$/i)).toBeDefined());

    act(() => {
      appCommands.closeDrawer();
    });
    await waitFor(() => expect(screen.queryByLabelText(/^sessions$/i)).toBeNull());
  });
});
