import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { MonitorPane } from "./MonitorPane";
import type { SessionView } from "../api/types";
import { useSessionStore } from "../state/SessionStore";
import { useTabStore } from "../state/TabStore";
import { appStatePayload, jsonResponse } from "../test/appState";

const STARTED_AT = "2026-05-02T00:00:00Z";
const ENDED_AT = "2026-05-02T00:00:02Z";
const AGENT_SESSION_UUID = "00000000-0000-0000-0000-000000000001";
const ALPHA_TASK = "Alpha task";

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
    label: id === "sess-a" ? ALPHA_TASK : null,
    pinned: false,
    color: null,
    future_prompts_pending_count: 0,
  };
}

describe("MonitorPane", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("loads every live terminal even when it has no open tab", async () => {
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
              label: ALPHA_TASK,
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
                session_label: ALPHA_TASK,
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

    render(<MonitorPane />);

    const report = await screen.findByTestId("monitor-report-sess-a");
    const user = userEvent.setup();
    await user.hover(report);
    await waitFor(() =>
      expect(screen.getByText("latest assistant output")).toBeDefined(),
    );
    expect(screen.getByText("sess-b")).toBeDefined();
    expect(bodies[0]).toMatchObject({
      session_ids: ["sess-a", "sess-b"],
      show_bookkeeping: false,
      show_sidechain: false,
    });
  });

  it("groups engineers by repo and shows measured health metrics", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo) => {
        const url = typeof input === "string" ? input : input.url;
        if (url === "/api/app-state") {
          return jsonResponse(
            appStatePayload({ sessions: useSessionStore.getState().sessions }),
          );
        }
        return jsonResponse({ generated_at: STARTED_AT, sessions: [] });
      }),
    );
    useSessionStore.setState({
      sessions: [
        {
          ...session("sess-a", 1),
          agent_runtime: {
            agent: "codex",
            state: "running",
            started_at: STARTED_AT,
            ended_at: null,
            exit_code: null,
          },
          agent_metadata: {
            agent: "codex",
            model: "gpt-5.4",
            model_provider: "openai",
            reasoning_effort: "medium",
            cli_version: "1.0.0",
            cwd: "/tmp/alpha",
            model_context_window: 100_000,
            updated_at: STARTED_AT,
          },
          agent_usage: {
            input_tokens: 42_000,
            cached_input_tokens: 31_000,
            output_tokens: 5_000,
            reasoning_output_tokens: 1_800,
            total_tokens: 47_000,
            context_tokens: 80_000,
            model_context_window: 100_000,
            observed_at: STARTED_AT,
            updated_at: STARTED_AT,
          },
          activity: {
            state: "working",
            summary: "Implementing the dashboard",
            reason: null,
            source: "agent",
            confidence: "explicit",
            updated_at: STARTED_AT,
          },
        },
        { ...session("sess-b", 1), repo: "beta", working_dir: "/tmp/beta" },
      ],
      sessionsLoaded: true,
    });

    render(<MonitorPane />);

    const alpha = await screen.findByRole("region", { name: "alpha team" });
    const beta = screen.getByRole("region", { name: "beta team" });
    expect(within(alpha).getByText(ALPHA_TASK)).toBeDefined();
    expect(within(beta).getByText("sess-b")).toBeDefined();
    expect(within(alpha).getByText("20%")).toBeDefined();
    expect(within(alpha).getByText("47K")).toBeDefined();
    expect(within(alpha).getByText("Implementing the dashboard")).toBeDefined();

    const user = userEvent.setup();
    await user.hover(within(alpha).getByTestId("monitor-ctx-sess-a"));
    await waitFor(() =>
      expect(
        screen.getByText(/80K of 100K used · 20% left/),
      ).toBeDefined(),
    );
  });

  it("shows attention state and opens a terminal's published plan", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo) => {
        const url = typeof input === "string" ? input : input.url;
        if (url === "/api/app-state") {
          return jsonResponse(
            appStatePayload({
              sessions: useSessionStore.getState().sessions,
              plans: useSessionStore.getState().plans,
            }),
          );
        }
        return jsonResponse({ generated_at: STARTED_AT, sessions: [] });
      }),
    );
    useSessionStore.setState({
      sessions: [
        {
          ...session("sess-a", 1),
          agent_runtime: {
            agent: "codex",
            state: "running",
            started_at: STARTED_AT,
            ended_at: null,
            exit_code: null,
          },
          activity: {
            state: "needs_input",
            summary: "Choose the durable API",
            reason: null,
            source: "agent",
            confidence: "explicit",
            updated_at: STARTED_AT,
          },
          current_plan: {
            id: "plan-1",
            title: "Native plans",
            status: "active",
            revision: 3,
            total_phases: 3,
            completed_phases: 1,
            current_phase_id: "phase-2",
            current_phase_title: "Frontend",
            current_phase_status: "blocked",
          },
        },
      ],
      plans: [
        {
          id: "plan-1",
          repo_name: "alpha",
          title: "Native plans",
          summary: "Publish work",
          status: "active",
          revision: 3,
          total_phases: 3,
          completed_phases: 1,
          blocked_phases: 1,
          current_phase_id: "phase-2",
          current_phase_title: "Frontend",
          current_phase_status: "blocked",
          attached_pty_ids: ["sess-a"],
          updated_at: STARTED_AT,
        },
      ],
      sessionsLoaded: true,
    });

    render(<MonitorPane />);
    expect(await screen.findByText("Choose the durable API")).toBeDefined();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /plan · native plans/i }));

    expect(
      Object.values(useTabStore.getState().tabs).some(
        (tab) => tab.kind === "plan" && tab.planId === "plan-1",
      ),
    ).toBe(true);
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
              label: ALPHA_TASK,
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
