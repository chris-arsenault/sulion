import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { MonitorPane } from "./MonitorPane";
import type { SessionView } from "../api/types";
import { useSessionStore } from "../state/SessionStore";
import { useTabStore } from "../state/TabStore";
import { appStatePayload, jsonResponse } from "../test/appState";

const STARTED_AT = "2026-05-02T00:00:00Z";
const ENDED_AT = "2026-05-02T00:00:02Z";
const AGENT_SESSION_UUID = "00000000-0000-0000-0000-000000000001";

function session(id: string, revision: number): SessionView {
  return {
    id,
    repo: "alpha",
    working_dir: "/tmp/alpha",
    state: "live",
    created_at: STARTED_AT,
    ended_at: null,
    exit_code: null,
    current_session_uuid: `${id}-agent`,
    current_session_agent: "codex",
    last_event_at: STARTED_AT,
    timeline_revision: revision,
    label: id === "sess-a" ? "Alpha task" : null,
    pinned: false,
    color: null,
    future_prompts_pending_count: 0,
  };
}

describe("MonitorPane", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("loads one latest-turn card per open session tab", async () => {
    const bodies: unknown[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.url;
        if (url === "/api/app-state") {
          return jsonResponse(appStatePayload({ sessions: useSessionStore.getState().sessions }));
        }
        bodies.push(JSON.parse(init?.body as string));
        return jsonResponse({
          generated_at: STARTED_AT,
          sessions: [
            {
              pty_session_id: "sess-a",
              repo: "alpha",
              label: "Alpha task",
              pty_state: "live",
              current_session_uuid: AGENT_SESSION_UUID,
              current_session_agent: "codex",
              total_event_count: 3,
              turn: {
                id: 7,
                preview: "inspect it",
                user_prompt_text: "inspect it",
                start_timestamp: STARTED_AT,
                end_timestamp: ENDED_AT,
                duration_ms: 2000,
                event_count: 3,
                operation_count: 0,
                tool_pairs: [],
                thinking_count: 0,
                has_errors: false,
                markdown: "",
                chunks: [
                  {
                    kind: "assistant",
                    items: [{ kind: "text", text: "latest assistant output" }],
                    thinking: [],
                  },
                ],
                pty_session_id: "sess-a",
                session_uuid: AGENT_SESSION_UUID,
                session_agent: "codex",
                session_label: "Alpha task",
                session_state: "live",
              },
            },
          ],
        });
      }),
    );

    useSessionStore.setState({
      sessions: [session("sess-a", 1), session("sess-b", 2)],
      sessionsLoaded: true,
    });
    useTabStore.getState().openTab({ kind: "terminal", sessionId: "sess-a" }, "top");
    useTabStore.getState().openTab({ kind: "timeline", sessionId: "sess-a" }, "bottom");
    useTabStore.getState().openTab({ kind: "terminal", sessionId: "sess-b" }, "top");

    render(<MonitorPane />);

    await waitFor(() => expect(screen.getByText("latest assistant output")).toBeDefined());
    expect(bodies[0]).toMatchObject({
      session_ids: ["sess-a", "sess-b"],
      show_bookkeeping: false,
      show_sidechain: false,
    });
  });

  it("opens the focused timeline for a monitor card", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo) => {
        const url = typeof input === "string" ? input : input.url;
        if (url === "/api/app-state") {
          return jsonResponse(appStatePayload({ sessions: useSessionStore.getState().sessions }));
        }
        return jsonResponse({
          generated_at: STARTED_AT,
          sessions: [
            {
              pty_session_id: "sess-a",
              repo: "alpha",
              label: "Alpha task",
              pty_state: "live",
              current_session_uuid: AGENT_SESSION_UUID,
              current_session_agent: "codex",
              total_event_count: 3,
              turn: {
                id: 7,
                preview: "inspect it",
                user_prompt_text: "inspect it",
                start_timestamp: STARTED_AT,
                end_timestamp: ENDED_AT,
                duration_ms: 2000,
                event_count: 3,
                operation_count: 0,
                tool_pairs: [],
                thinking_count: 0,
                has_errors: false,
                markdown: "",
                chunks: [],
              },
            },
          ],
        });
      }),
    );

    useSessionStore.setState({
      sessions: [session("sess-a", 1)],
      sessionsLoaded: true,
    });
    useTabStore.getState().openTab({ kind: "terminal", sessionId: "sess-a" }, "top");

    render(<MonitorPane />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /open timeline/i }));

    const timeline = Object.values(useTabStore.getState().tabs).find(
      (tab) => tab.kind === "timeline" && tab.sessionId === "sess-a",
    );
    expect(timeline?.focusTurnId).toBe(7);
  });
});
