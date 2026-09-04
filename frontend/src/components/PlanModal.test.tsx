import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { PlanView, SessionView } from "../api/types";
import { resetSessionStore, useSessionStore } from "../state/SessionStore";
import { resetTabStore } from "../state/TabStore";
import {
  type AppCommand,
  resetAppCommands,
  subscribeToAppCommand,
} from "../state/AppCommands";
import { appStatePayload, jsonResponse } from "../test/appState";
import { PlanModal } from "./PlanModal";

const NOW = "2026-07-23T00:00:00Z";
const ROOT_TITLE = "Native plans";
const APP_STATE_URL = "/api/app-state";
const noop = () => {};

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
    title: ROOT_TITLE,
    summary: "Publish durable phases",
    status,
    revision: 2,
    parent_plan_id: null,
    root_plan_id: "plan-1",
    depth: 0,
    created_by_pty_id: null,
    created_by_agent_session_uuid: null,
    created_at: NOW,
    updated_at: NOW,
    closed_at: null,
    anchor_phase_ids: [],
    ancestors: [],
    branches: [],
    phases: [
      {
        id: "phase-1",
        plan_id: "plan-1",
        position: 1,
        title: "Backend",
        description: "Schema and service",
        status: "in_progress",
        status_note: null,
        size: null,
        started_at: NOW,
        completed_at: null,
        created_at: NOW,
        updated_at: NOW,
      },
    ],
    attachments: [],
  };
}

describe("PlanModal", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    resetAppCommands();
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
        if (url === APP_STATE_URL) {
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
        if (url.endsWith("/events")) return jsonResponse([]);
        // Detail load after creation resolves to the created plan.
        if (url.endsWith("/plans/plan-1")) return jsonResponse(created);
        return jsonResponse([]);
      }),
    );
    useSessionStore.setState({
      sessions: [liveSession()],
      sessionsLoaded: true,
    });

    render(<PlanModal open repo="alpha" onClose={noop} />);
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Title"), ROOT_TITLE);
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
        title: ROOT_TITLE,
        summary: "Publish durable phases",
        phases: [
          { title: "Backend", description: "Schema and service" },
          { title: "Frontend", description: "Plan workspace" },
        ],
        attach_pty_id: "pty-1",
      }),
    );
    // Creating navigates the modal onto the new plan's detail view.
    expect(await screen.findByText("revision 2")).toBeDefined();
  });

  it("updates a published phase from the plan detail view", async () => {
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
        if (url === APP_STATE_URL) return jsonResponse(appStatePayload());
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

    render(<PlanModal open repo="alpha" planId="plan-1" onClose={noop} />);
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Edit phase 1" }),
    );
    await user.selectOptions(
      screen.getByLabelText("Phase 1 status"),
      "completed",
    );
    await user.type(screen.getByLabelText("Phase 1 status note"), "verified");
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

  it("opens a file:line reference from a phase description", async () => {
    const withRef: PlanView = {
      ...plan(),
      phases: [
        {
          ...plan().phases[0]!,
          description: "Wire it in src/state/TabStore.tsx:441 first.",
        },
      ],
    };
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo) => {
        const url = typeof input === "string" ? input : input.url;
        if (url === APP_STATE_URL) return jsonResponse(appStatePayload());
        if (url.endsWith("/events")) return jsonResponse([]);
        return jsonResponse(withRef);
      }),
    );
    const commands: AppCommand[] = [];
    subscribeToAppCommand("open-file", (command) => commands.push(command));

    render(<PlanModal open repo="alpha" planId="plan-1" onClose={noop} />);
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", {
        name: "src/state/TabStore.tsx:441",
      }),
    );

    expect(commands).toEqual([
      {
        type: "open-file",
        repo: "alpha",
        path: "src/state/TabStore.tsx",
        workspaceId: undefined,
        line: 441,
      },
    ]);
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
        if (url === APP_STATE_URL) return jsonResponse(appStatePayload());
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

    render(<PlanModal open repo="alpha" planId="plan-1" onClose={noop} />);
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

  it("branches from a phase over a span and opens the new sub-plan", async () => {
    const root = plan();
    const branch: PlanView = {
      ...plan(),
      id: "plan-2",
      title: "Unblock the gate",
      parent_plan_id: "plan-1",
      root_plan_id: "plan-1",
      depth: 1,
      anchor_phase_ids: ["phase-1"],
      ancestors: [
        { id: "plan-1", title: ROOT_TITLE, status: "active", depth: 0 },
      ],
    };
    const requests: Array<{ url: string; method: string; body?: unknown }> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.url;
        const method = init?.method ?? "GET";
        if (url === APP_STATE_URL) return jsonResponse(appStatePayload());
        if (url.endsWith("/events")) return jsonResponse([]);
        requests.push({
          url,
          method,
          body: init?.body ? JSON.parse(init.body as string) : undefined,
        });
        if (method === "POST") return jsonResponse(branch, 201);
        if (url.endsWith("/plans/plan-2")) return jsonResponse(branch);
        return jsonResponse(root);
      }),
    );

    render(<PlanModal open repo="alpha" planId="plan-1" onClose={noop} />);
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Branch from phase 1" }),
    );
    await user.type(screen.getByLabelText("Sub-plan title"), "Unblock the gate");
    await user.clear(screen.getByLabelText("Parent phases covered"));
    await user.type(screen.getByLabelText("Parent phases covered"), "1,2,3");
    await user.type(
      screen.getByLabelText("Sub-plan phases"),
      "Diagnose | Find the cause{enter}Fix | Land it",
    );
    await user.click(screen.getByRole("button", { name: "Branch" }));

    await waitFor(() =>
      expect(requests.find((request) => request.method === "POST")).toMatchObject(
        {
          url: "/api/plans/plan-1/branches",
          body: {
            title: "Unblock the gate",
            parent_phase_refs: ["1", "2", "3"],
            phases: [
              { title: "Diagnose", description: "Find the cause" },
              { title: "Fix", description: "Land it" },
            ],
          },
        },
      ),
    );
    // The modal follows the work onto the branch and shows the way back.
    expect(
      await screen.findByRole("button", { name: ROOT_TITLE }),
    ).toBeDefined();
  });

  it("returns a branch to its parent instead of closing it outright", async () => {
    const branch: PlanView = {
      ...plan(),
      id: "plan-2",
      title: "Unblock the gate",
      parent_plan_id: "plan-1",
      root_plan_id: "plan-1",
      depth: 1,
      anchor_phase_ids: ["phase-1"],
      ancestors: [
        { id: "plan-1", title: ROOT_TITLE, status: "active", depth: 0 },
      ],
    };
    const parent = plan();
    const requests: Array<{ url: string; method: string; body?: unknown }> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo, init?: RequestInit) => {
        const url = typeof input === "string" ? input : input.url;
        const method = init?.method ?? "GET";
        if (url === APP_STATE_URL) return jsonResponse(appStatePayload());
        if (url.endsWith("/events")) return jsonResponse([]);
        requests.push({
          url,
          method,
          body: init?.body ? JSON.parse(init.body as string) : undefined,
        });
        if (url.endsWith("/plans/plan-1")) return jsonResponse(parent);
        return jsonResponse(branch);
      }),
    );

    render(<PlanModal open repo="alpha" planId="plan-2" onClose={noop} />);
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Return & skip 1" }),
    );

    await waitFor(() =>
      expect(
        requests.find((request) => request.method === "PATCH"),
      ).toMatchObject({
        url: "/api/plans/plan-2",
        body: { status: "completed", skip_remaining: true },
      }),
    );
    // Returning lands on the parent, which is a root and so has no breadcrumb.
    await waitFor(() => expect(screen.getByText("Backend")).toBeDefined());
    expect(document.querySelector(".plan-modal__breadcrumb")).toBeNull();
  });
});
