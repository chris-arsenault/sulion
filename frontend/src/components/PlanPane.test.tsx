import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { PlanView, SessionView } from "../api/types";
import {
  resetSessionStore,
  useSessionStore,
} from "../state/SessionStore";
import { resetTabStore, useTabStore } from "../state/TabStore";
import { appStatePayload, jsonResponse } from "../test/appState";
import { PlanPane } from "./PlanPane";

const NOW = "2026-07-23T00:00:00Z";

function liveSession(): SessionView {
  return {
    id: "pty-1",
    repo: "alpha",
    working_dir: "/tmp/alpha",
    state: "live",
    created_at: NOW,
    ended_at: null,
    exit_code: null,
    current_session_uuid: null,
    current_session_agent: null,
    last_event_at: null,
    label: "Frontend agent",
    pinned: false,
    color: null,
    future_prompts_pending_count: 0,
  };
}

function plan(status: PlanView["status"] = "active"): PlanView {
  return {
    id: "plan-1",
    repo_name: "alpha",
    title: "Native plans",
    summary: "Publish durable phases",
    status,
    revision: 2,
    created_by_pty_id: null,
    created_by_agent_session_uuid: null,
    created_at: NOW,
    updated_at: NOW,
    closed_at: null,
    phases: [
      {
        id: "phase-1",
        plan_id: "plan-1",
        position: 1,
        title: "Backend",
        description: "Schema and service",
        status: "in_progress",
        status_note: null,
        started_at: NOW,
        completed_at: null,
        created_at: NOW,
        updated_at: NOW,
      },
    ],
    attachments: [],
  };
}

describe("PlanPane", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    act(() => {
      resetSessionStore();
      resetTabStore();
    });
  });

  it("starts a plan with parsed phases and an optional terminal attachment", async () => {
    const requests: Array<{ url: string; method: string; body?: unknown }> = [];
    const created = plan();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.url;
        const method = init?.method ?? "GET";
        if (url === "/api/app-state") {
          return jsonResponse(
            appStatePayload({ sessions: useSessionStore.getState().sessions }),
          );
        }
        requests.push({
          url,
          method,
          body: init?.body ? JSON.parse(init.body as string) : undefined,
        });
        if (method === "POST") return jsonResponse(created, 201);
        return jsonResponse([]);
      }),
    );
    useSessionStore.setState({
      sessions: [liveSession()],
      sessionsLoaded: true,
    });

    render(<PlanPane repo="alpha" />);
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Title"), "Native plans");
    await user.type(
      screen.getByLabelText("Short description"),
      "Publish durable phases",
    );
    await user.type(
      screen.getByLabelText("Phases"),
      "Backend | Schema and service{enter}Frontend | Plan workspace",
    );
    await user.selectOptions(
      screen.getByLabelText("Attach to terminal"),
      "pty-1",
    );
    await user.click(screen.getByRole("button", { name: "Start plan" }));

    await waitFor(() =>
      expect(
        requests.find((request) => request.method === "POST")?.body,
      ).toEqual({
        title: "Native plans",
        summary: "Publish durable phases",
        phases: [
          { title: "Backend", description: "Schema and service" },
          { title: "Frontend", description: "Plan workspace" },
        ],
        attach_pty_id: "pty-1",
      }),
    );
    expect(
      Object.values(useTabStore.getState().tabs).some(
        (tab) => tab.kind === "plan" && tab.planId === "plan-1",
      ),
    ).toBe(true);
  });

  it("updates a published phase from the plan detail workspace", async () => {
    const initial = plan();
    const completed: PlanView = {
      ...initial,
      revision: 3,
      phases: [
        {
          ...initial.phases[0]!,
          status: "completed",
          status_note: "verified",
          completed_at: NOW,
        },
      ],
    };
    const requests: Array<{ url: string; method: string; body?: unknown }> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.url;
        const method = init?.method ?? "GET";
        if (url === "/api/app-state") return jsonResponse(appStatePayload());
        requests.push({
          url,
          method,
          body: init?.body ? JSON.parse(init.body as string) : undefined,
        });
        if (url.endsWith("/events")) return jsonResponse([]);
        if (method === "PATCH") return jsonResponse(completed);
        return jsonResponse(initial);
      }),
    );

    render(<PlanPane repo="alpha" planId="plan-1" />);
    const user = userEvent.setup();
    await screen.findByDisplayValue("Schema and service");
    await user.selectOptions(
      screen.getByLabelText("Phase 1 status"),
      "completed",
    );
    await user.type(
      screen.getByLabelText("Phase 1 status note"),
      "verified",
    );
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(
        requests.find((request) => request.method === "PATCH")?.body,
      ).toMatchObject({
        status: "completed",
        status_note: "verified",
      }),
    );
    expect(screen.getByText("revision 3")).toBeDefined();
  });

  it("explicitly skips unfinished phases when completing a plan", async () => {
    const initial = plan();
    const completed: PlanView = {
      ...initial,
      status: "completed",
      revision: 3,
      closed_at: NOW,
      phases: [
        {
          ...initial.phases[0]!,
          status: "skipped",
          completed_at: NOW,
        },
      ],
    };
    const requests: Array<{ method: string; body?: unknown }> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.url;
        const method = init?.method ?? "GET";
        if (url === "/api/app-state") return jsonResponse(appStatePayload());
        if (url.endsWith("/events")) return jsonResponse([]);
        if (method === "PATCH") {
          requests.push({
            method,
            body: init?.body ? JSON.parse(init.body as string) : undefined,
          });
          return jsonResponse(completed);
        }
        return jsonResponse(initial);
      }),
    );

    render(<PlanPane repo="alpha" planId="plan-1" />);
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Complete & skip 1" }),
    );

    await waitFor(() =>
      expect(requests[0]?.body).toEqual({
        status: "completed",
        skip_remaining: true,
      }),
    );
    expect(
      document.querySelector(".plan-status--completed")?.textContent,
    ).toBe("completed");
  });
});
