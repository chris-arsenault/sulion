import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { MonitorPane } from "./MonitorPane";
import { ContextMenuHost } from "./common/ContextMenu";
import { resetContextMenuStore } from "./common/contextMenuStore";
import type { SessionView } from "../api/types";
import { useSessionStore } from "../state/SessionStore";
import { useTabStore } from "../state/TabStore";
import { appCommands } from "../state/AppCommands";
import { appStatePayload, jsonResponse } from "../test/appState";

const STARTED_AT = "2026-05-02T00:00:00Z";
const ENDED_AT = "2026-05-02T00:00:02Z";
const AGENT_SESSION_UUID = "00000000-0000-0000-0000-000000000001";
const ALPHA_TASK = "Alpha task";
const APP_STATE_URL = "/api/app-state";

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
    vi.restoreAllMocks();
    resetContextMenuStore();
  });

  it("loads every live terminal even when it has no open tab", async () => {
    const bodies: unknown[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.url;
        if (url === APP_STATE_URL) {
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
        if (url === APP_STATE_URL) {
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
          agent_label: "batcher dedupe pass",
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
    // The agent-set name sits beside the user's label on the card.
    expect(within(alpha).getByText("batcher dedupe pass")).toBeDefined();
    expect(within(beta).getByText("sess-b")).toBeDefined();
    expect(within(alpha).getByText("20%")).toBeDefined();
    // Fresh tokens (total 47K − 31K cache reads) headline the card.
    expect(within(alpha).getByText("16K")).toBeDefined();
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
        if (url === APP_STATE_URL) {
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

    const openPlan = vi.spyOn(appCommands, "openPlan");
    render(<MonitorPane />);
    expect(await screen.findByText("Choose the durable API")).toBeDefined();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /plan · native plans/i }));

    expect(openPlan).toHaveBeenCalledWith({ repo: "alpha", planId: "plan-1" });
  });

  it("offers Go to terminal from the card context menu and closes the modal host", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo) => {
        const url = typeof input === "string" ? input : input.url;
        if (url === APP_STATE_URL) {
          return jsonResponse(
            appStatePayload({ sessions: useSessionStore.getState().sessions }),
          );
        }
        return jsonResponse({ generated_at: STARTED_AT, sessions: [] });
      }),
    );
    useSessionStore.setState({
      sessions: [session("sess-ctx", 1)],
      sessionsLoaded: true,
    });

    const onNavigate = vi.fn();
    render(
      <>
        <MonitorPane onNavigate={onNavigate} />
        <ContextMenuHost />
      </>,
    );
    const user = userEvent.setup();
    await user.pointer({
      keys: "[MouseRight]",
      target: await screen.findByText("sess-ctx"),
    });
    await user.click(
      await screen.findByRole("menuitem", { name: "Go to terminal" }),
    );

    const tabs = Object.values(useTabStore.getState().tabs);
    expect(
      tabs.some((tab) => tab.kind === "terminal" && tab.sessionId === "sess-ctx"),
    ).toBe(true);
    expect(onNavigate).toHaveBeenCalled();
  });

  it("offers repo actions from the team header context menu", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo) => {
        const url = typeof input === "string" ? input : input.url;
        if (url === APP_STATE_URL) {
          return jsonResponse(
            appStatePayload({ sessions: useSessionStore.getState().sessions }),
          );
        }
        return jsonResponse({ generated_at: STARTED_AT, sessions: [] });
      }),
    );
    useSessionStore.setState({
      sessions: [session("sess-hdr", 1)],
      sessionsLoaded: true,
    });

    const openPlan = vi.spyOn(appCommands, "openPlan");
    render(
      <>
        <MonitorPane />
        <ContextMenuHost />
      </>,
    );
    const user = userEvent.setup();
    const region = await screen.findByRole("region", { name: "alpha team" });
    await user.pointer({
      keys: "[MouseRight]",
      target: within(region).getByRole("heading", { name: "alpha" }),
    });
    await user.click(
      await screen.findByRole("menuitem", { name: "Open published plans" }),
    );

    expect(openPlan).toHaveBeenCalledWith({ repo: "alpha" });
  });

  it("shows an abandoned card for orphaned resumable sessions and resumes on click", async () => {
    const created = {
      ...session("sess-new", 1),
      id: "sess-new",
    };
    const posts: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.url;
        if (url === APP_STATE_URL) {
          return jsonResponse(
            appStatePayload({ sessions: useSessionStore.getState().sessions }),
          );
        }
        if (url === "/api/sessions" && init?.method === "POST") {
          posts.push(init.body as string);
          return jsonResponse(created);
        }
        if (url.startsWith("/api/sessions/") && init?.method === "DELETE") {
          return jsonResponse({});
        }
        return jsonResponse({ generated_at: STARTED_AT, sessions: [] });
      }),
    );
    useSessionStore.setState({
      sessions: [
        {
          ...session("sess-gone", 1),
          state: "orphaned",
          label: "Abandoned work",
          current_session_uuid: AGENT_SESSION_UUID,
          current_session_agent: "claude-code",
        },
      ],
      sessionsLoaded: true,
    });

    const onNavigate = vi.fn();
    render(<MonitorPane onNavigate={onNavigate} />);

    const region = await screen.findByRole("region", { name: "alpha team" });
    expect(within(region).getByText(/orphaned claude-code session/)).toBeDefined();

    const user = userEvent.setup();
    await user.click(
      within(region).getByRole("button", {
        name: "Resume Abandoned work with a new PTY",
      }),
    );

    await waitFor(() => expect(posts.length).toBe(1));
    expect(JSON.parse(posts[0]!)).toMatchObject({
      repo: "alpha",
      resume_session_uuid: AGENT_SESSION_UUID,
      resume_agent: "claude-code",
    });
    await waitFor(() =>
      expect(
        Object.values(useTabStore.getState().tabs).some(
          (tab) => tab.kind === "terminal" && tab.sessionId === "sess-new",
        ),
      ).toBe(true),
    );
    expect(onNavigate).toHaveBeenCalled();
  });

  it("does not offer resume for dead sessions", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo) => {
        const url = typeof input === "string" ? input : input.url;
        if (url === APP_STATE_URL) {
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
          ...session("sess-dead", 1),
          state: "dead",
          current_session_uuid: AGENT_SESSION_UUID,
          current_session_agent: "claude-code",
        },
      ],
      sessionsLoaded: true,
    });

    render(<MonitorPane />);
    await waitFor(() =>
      expect(screen.getByText(/no live teams or published plans/i)).toBeDefined(),
    );
    expect(screen.queryByRole("button", { name: /resume/i })).toBeNull();
  });

  it("opens the focused timeline for a monitor card", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo) => {
        const url = typeof input === "string" ? input : input.url;
        if (url === APP_STATE_URL) {
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
    await user.click(
      await screen.findByRole("button", { name: /go to terminal for/i }),
    );

    // The card's primary click raises the session: terminal in the top
    // pane plus the timeline focused on the latest turn below.
    const tabs = useTabStore.getState();
    const terminal = Object.values(tabs.tabs).find(
      (tab) => tab.kind === "terminal" && tab.sessionId === "sess-a",
    );
    expect(terminal).toBeDefined();
    expect(tabs.activeByPane.top).toBe(terminal?.id);
    const timeline = Object.values(tabs.tabs).find(
      (tab) => tab.kind === "timeline" && tab.sessionId === "sess-a",
    );
    expect(timeline?.focusTurnId).toBe(7);
  });
});
