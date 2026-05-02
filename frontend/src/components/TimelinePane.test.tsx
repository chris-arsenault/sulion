import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("react-virtuoso", () => ({
  Virtuoso: ({
    data,
    itemContent,
  }: {
    data: unknown[];
    itemContent: (i: number, item: unknown) => React.ReactNode;
  }) => (
    <div data-testid="virtuoso">
      {data.map((item, idx) => (
        <div key={idx}>{itemContent(idx, item)}</div>
      ))}
    </div>
  ),
}));

import { TimelinePane as TimelinePaneRaw } from "./TimelinePane";
import { ContextMenuHost } from "./common/ContextMenu";

const TimelinePane = (props: { sessionId?: string; repo?: string }) => (
  <>
    <TimelinePaneRaw {...props} />
    <ContextMenuHost />
  </>
);

function stubFetch(handler: (url: string, init?: RequestInit) => Response) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo, init?: RequestInit) => {
      const url = typeof input === "string" ? input : input.url;
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

function timelineBody(overrides: Partial<Record<string, unknown>> = {}) {
  return JSON.stringify(timelinePayload(overrides));
}

function timelinePayload(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    session_uuid: "00000000-0000-0000-0000-000000000001",
    session_agent: "claude-code",
    total_event_count: 2,
    turns: [
      {
        id: 1,
        preview: "hello",
        user_prompt_text: "hello",
        start_timestamp: "2025-01-01T00:00:00Z",
        end_timestamp: "2025-01-01T00:00:02Z",
        duration_ms: 2000,
        event_count: 2,
        operation_count: 0,
        operation_badges: [],
        tool_pairs: [],
        thinking_count: 0,
        has_errors: false,
        markdown: "**Prompt**\n\n> hello",
        chunks: [{ kind: "assistant", items: [{ kind: "text", text: "hi there" }], thinking: [] }],
        turn_key: "00000000-0000-0000-0000-000000000001:1",
        pty_session_id: "abc",
        session_uuid: "00000000-0000-0000-0000-000000000001",
        session_agent: "claude-code",
        session_label: "investigation",
        session_state: "dead",
      },
    ],
    ...overrides,
  };
}

function timelineDetailBody(turn: Record<string, unknown>) {
  return JSON.stringify({
    session_uuid:
      turn.session_uuid ?? "00000000-0000-0000-0000-000000000001",
    session_agent: turn.session_agent ?? "claude-code",
    turn,
  });
}

describe("TimelinePane", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    window.localStorage.clear();
  });

  it("renders projected turns and opens the inspector on click", async () => {
    const payload = timelinePayload();
    const turn = payload.turns[0] as Record<string, unknown>;
    stubFetch((url) =>
      new Response(
        url.includes("/timeline/turns/") ? timelineDetailBody(turn) : JSON.stringify(payload),
        {
          status: 200,
          headers: { "Content-Type": "application/json" },
        },
      ),
    );

    render(<TimelinePane sessionId="abc" />);
    await waitFor(() => expect(screen.getByText(/hello/)).toBeDefined());
    expect(screen.getByText(/1 turn/)).toBeDefined();

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /hello/ }));
    await waitFor(() => expect(screen.getByText("hi there")).toBeDefined());
  });

  it("shows empty-state copy when the API returns no correlated session", async () => {
    stubFetch(() => new Response(JSON.stringify({
      session_uuid: null,
      session_agent: null,
      total_event_count: 0,
      turns: [],
    }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    }));

    render(<TimelinePane sessionId="abc" />);
    await waitFor(() => {
      expect(screen.getByText(/no transcript session correlated yet/i)).toBeDefined();
    });
  });

  it("polls the timeline endpoint rather than history", async () => {
    const urls: string[] = [];
    stubFetch((url) => {
      urls.push(url);
      return new Response(timelineBody(), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    });

    render(<TimelinePane sessionId="abc" />);
    await waitFor(() => expect(screen.getByText(/hello/)).toBeDefined());
    expect(urls[0]).toMatch(/\/api\/sessions\/abc\/timeline/);
  });

  it("surfaces projected event counts in the header", async () => {
    stubFetch(() => new Response(timelineBody({ total_event_count: 3 }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    }));

    render(<TimelinePane sessionId="abc" />);
    await waitFor(() => expect(screen.getByText(/1 turn/)).toBeDefined());
    expect(screen.getByText(/3 events/)).toBeDefined();
  });

  it("keeps the user's manual turn selection across poll refreshes", async () => {
    const makeTurn = (
      id: number,
      preview: string,
      assistantText: string,
    ) => ({
      id,
      preview,
      user_prompt_text: preview,
      start_timestamp: "2025-01-01T00:00:00Z",
      end_timestamp: "2025-01-01T00:00:02Z",
      duration_ms: 2000,
      event_count: 2,
      operation_count: 0,
      operation_badges: [],
      tool_pairs: [],
      thinking_count: 0,
      has_errors: false,
      markdown: `**Prompt**\n\n> ${preview}`,
      chunks: [
        {
          kind: "assistant",
          items: [{ kind: "text", text: assistantText }],
          thinking: [],
        },
      ],
      turn_key: `00000000-0000-0000-0000-000000000001:${id}`,
      session_uuid: "00000000-0000-0000-0000-000000000001",
    });
    const turns = [
      makeTurn(1, "hello", "hi there"),
      makeTurn(2, "goodbye", "later content"),
    ];

    stubFetch((url) =>
      new Response(
        url.includes("/timeline/turns/")
          ? timelineDetailBody(
              turns.find((turn) => url.includes(`/turns/${turn.id}`)) ?? turns[0]!,
            )
          : timelineBody({ turns }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
    );

    render(<TimelinePaneRaw sessionId="abc" focusTurnId={1} focusKey="k1" />);

    // focusKey effect applies after the first poll response, selecting turn 1.
    await waitFor(() => expect(screen.getByText("hi there")).toBeDefined());

    // User clicks turn 2 — inspector swaps to its content.
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /goodbye/ }));
    await waitFor(() =>
      expect(screen.getByText("later content")).toBeDefined(),
    );

    // Let at least one poll cycle complete so `turns` gets a fresh array
    // identity. Under the original bug this re-ran the focus effect and
    // snapped the selection back to turn 1.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 1800));
    });

    expect(screen.getByText("later content")).toBeDefined();
    expect(screen.queryByText("hi there")).toBeNull();
  });

  it("refetches the selected turn detail when its summary changes", async () => {
    let summaryCalls = 0;
    let detailCalls = 0;
    const makeTurn = (text: string, eventCount: number) => ({
      ...((timelinePayload().turns[0] as Record<string, unknown>) ?? {}),
      event_count: eventCount,
      chunks: [
        {
          kind: "assistant",
          items: [{ kind: "text", text }],
          thinking: [],
        },
      ],
    });

    stubFetch((url) => {
      if (url.includes("/timeline/turns/")) {
        detailCalls += 1;
        return new Response(
          timelineDetailBody(makeTurn(`detail ${detailCalls}`, summaryCalls + 1)),
          { status: 200, headers: { "Content-Type": "application/json" } },
        );
      }
      summaryCalls += 1;
      return new Response(
        timelineBody({
          turns: [makeTurn("summary only", summaryCalls + 1)],
        }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      );
    });

    const user = userEvent.setup();
    render(<TimelinePane sessionId="abc" />);
    await waitFor(() => expect(screen.getByText(/hello/)).toBeDefined());

    await user.click(screen.getByRole("button", { name: /hello/ }));
    await waitFor(() => expect(screen.getByText("detail 1")).toBeDefined());

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 1800));
    });

    await waitFor(() => expect(screen.getByText("detail 2")).toBeDefined());
    expect(detailCalls).toBeGreaterThan(1);
  });

  it("fetches the repo timeline endpoint in repo mode", async () => {
    const urls: string[] = [];
    stubFetch((url) => {
      urls.push(url);
      return new Response(timelineBody({ session_uuid: null, session_agent: null }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    });

    render(<TimelinePane repo="alpha" />);
    await waitFor(() => expect(screen.getByText(/repo alpha/i)).toBeDefined());
    expect(urls[0]).toMatch(/\/api\/repos\/alpha\/timeline/);
  });

  it("adjusts timeline text size without changing the pane layout tokens", async () => {
    stubFetch(() => new Response(timelineBody(), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    }));

    const user = userEvent.setup();
    render(<TimelinePane sessionId="abc" />);

    await waitFor(() => expect(screen.getByText(/1 turn/)).toBeDefined());

    await user.click(screen.getByRole("button", { name: /increase timeline text size/i }));

    const pane = screen.getByTestId("timeline-pane");
    expect(pane.style.getPropertyValue("--timeline-t-ui")).toContain("1.1");
    expect(window.localStorage.getItem("sulion.timeline.font-scale.v1")).toBe("1.1");
  });
});
