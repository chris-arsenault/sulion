import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// Virtuoso does its own DOM measurement which happy-dom doesn't support.
// Stub it with a plain list so we can assert on rendered items directly.
vi.mock("react-virtuoso", () => ({
  Virtuoso: ({
    data,
    itemContent,
  }: {
    data: unknown[];
    itemContent: (i: number, item: unknown) => React.ReactNode;
  }) => (
    <div data-testid="virtuoso">
      {data.map((d, i) => (
        <div key={i}>{itemContent(i, d)}</div>
      ))}
    </div>
  ),
}));

import { TimelinePane as TimelinePaneRaw } from "./TimelinePane";
import { ContextMenuHost } from "./common/ContextMenu";
import { makeEvent, textBlock } from "./timeline/test-helpers";

// TimelinePane now reads the session list for the current session's
// repo (used to scope the "Pin as reference" menu) and TurnRow uses
// the singleton context-menu store, so include the host layer.
const TimelinePane = (props: { sessionId: string }) => (
  <>
    <TimelinePaneRaw {...props} />
    <ContextMenuHost />
  </>
);

function stubHistoryFetch(
  handler: (url: string, init?: RequestInit) => Response,
) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo, init?: RequestInit) => {
      const url = typeof input === "string" ? input : input.url;
      // The wrapper providers fire mount-time polls; route those to
      // empty-JSON shells so they don't confuse the per-test handler.
      if (url === "/api/sessions") {
        return new Response(JSON.stringify({ sessions: [] }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      if (url === "/api/repos") {
        return new Response(JSON.stringify({ repos: [] }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      return handler(url, init);
    }),
  );
}

const mkUser = (offset: number, text: string) => ({
  ...makeEvent("user", {
    byte_offset: offset,
    timestamp: new Date(offset * 1000).toISOString(),
    blocks: [textBlock(0, text)],
  }),
});

const mkAssistant = (offset: number, text: string) => ({
  ...makeEvent("assistant", {
    byte_offset: offset,
    timestamp: new Date(offset * 1000).toISOString(),
    blocks: [textBlock(0, text)],
  }),
});

describe("TimelinePane", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    window.localStorage.clear();
  });

  it("groups events into turns and expands the header on click", async () => {
    const body = JSON.stringify({
      session_uuid: "00000000-0000-0000-0000-000000000001",
      session_agent: "claude-code",
      events: [mkUser(0, "hello"), mkAssistant(120, "hi there")],
      next_after: 120,
    });
    stubHistoryFetch(
      () =>
        new Response(body, {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
    );

    render(<TimelinePane sessionId="abc" />);

    await waitFor(() => {
      expect(screen.getByText(/hello/)).toBeDefined();
    });
    // Header shows turn + event counts
    expect(screen.getByText(/1 turn/)).toBeDefined();

    const user = userEvent.setup();
    // Click the turn header to expand and reveal the assistant text
    await user.click(screen.getByRole("button", { name: /hello/ }));
    await waitFor(() => {
      expect(screen.getByText("hi there")).toBeDefined();
    });
  });

  it("shows empty-state copy when the API returns no correlated session", async () => {
    const body = JSON.stringify({
      session_uuid: null,
      session_agent: null,
      events: [],
      next_after: null,
    });
    stubHistoryFetch(
      () =>
        new Response(body, {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
    );

    render(<TimelinePane sessionId="abc" />);
    await waitFor(() => {
      expect(screen.getByText(/no transcript session correlated yet/i)).toBeDefined();
    });
  });

  it("subsequent polls include ?after=<last_offset>", async () => {
    let call = 0;
    const urls: string[] = [];
    stubHistoryFetch((url) => {
      urls.push(url);
      const body =
        call++ === 0
          ? {
              session_uuid: "00000000-0000-0000-0000-000000000001",
              session_agent: "claude-code",
              events: [mkUser(0, "one")],
              next_after: 0,
            }
          : {
              session_uuid: "00000000-0000-0000-0000-000000000001",
              session_agent: "claude-code",
              events: [mkUser(1, "two")],
              next_after: 1,
            };
      return new Response(JSON.stringify(body), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    });

    render(<TimelinePane sessionId="abc" />);
    await waitFor(() => expect(screen.getByText(/one/)).toBeDefined());
    // Wait for the next poll. Poll interval is 1500ms; allow slack.
    await waitFor(() => expect(screen.getByText(/two/)).toBeDefined(), {
      timeout: 3500,
    });
    expect(urls[0]).toMatch(/\/api\/sessions\/abc\/history$/);
    expect(urls.some((u) => /after=0/.test(u))).toBe(true);
  });

  it("bookkeeping events are hidden by default", async () => {
    const body = JSON.stringify({
      session_uuid: "00000000-0000-0000-0000-000000000001",
      session_agent: "claude-code",
      events: [
        mkUser(0, "real prompt"),
        {
          ...makeEvent("file-history-snapshot", {
            byte_offset: 10,
            timestamp: "2025-01-01T00:00:10Z",
          }),
        },
        mkAssistant(20, "real reply"),
      ],
      next_after: 20,
    });
    stubHistoryFetch(
      () =>
        new Response(body, {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
    );

    render(<TimelinePane sessionId="abc" />);
    await waitFor(() => expect(screen.getByText(/real prompt/)).toBeDefined());
    // Header reports 3 raw events but only one visible turn
    expect(screen.getByText(/1 turn/)).toBeDefined();
    expect(screen.getByText(/3 events/)).toBeDefined();
    expect(screen.queryByText(/file-history-snapshot/)).toBeNull();
  });
});
