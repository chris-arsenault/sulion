import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { Sidebar } from "./Sidebar";
import { SessionProvider } from "../state/SessionStore";

type Endpoint = "/api/sessions" | "/api/repos" | string;

interface MockState {
  sessions: Array<Record<string, unknown>>;
  repos: Array<Record<string, unknown>>;
  createSessionCalls: Array<unknown>;
  createRepoCalls: Array<unknown>;
  deletedIds: string[];
}

function installFetchMock(): MockState {
  const state: MockState = {
    sessions: [],
    repos: [],
    createSessionCalls: [],
    createRepoCalls: [],
    deletedIds: [],
  };

  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo, init?: RequestInit) => {
      const url: Endpoint =
        typeof input === "string" ? input : (input as Request).url;
      const method = init?.method ?? "GET";
      const jsonResp = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        });

      if (url === "/api/sessions" && method === "GET") {
        return jsonResp({ sessions: state.sessions });
      }
      if (url === "/api/sessions" && method === "POST") {
        const body = JSON.parse(init!.body as string);
        state.createSessionCalls.push(body);
        const s = {
          id: `00000000-0000-0000-0000-${String(
            state.sessions.length,
          ).padStart(12, "0")}`,
          repo: body.repo,
          working_dir: body.working_dir ?? `/tmp/${body.repo}`,
          state: "live",
          created_at: new Date().toISOString(),
          ended_at: null,
          exit_code: null,
          current_claude_session_uuid: null,
        };
        state.sessions.push(s);
        return jsonResp(s, 201);
      }
      if (url.startsWith("/api/sessions/") && method === "DELETE") {
        const id = url.split("/").pop()!;
        state.sessions = state.sessions.filter((s) => s.id !== id);
        state.deletedIds.push(id);
        return new Response(null, { status: 204 });
      }
      if (url === "/api/repos" && method === "GET") {
        return jsonResp({ repos: state.repos });
      }
      if (url === "/api/repos" && method === "POST") {
        const body = JSON.parse(init!.body as string);
        state.createRepoCalls.push(body);
        const r = { name: body.name, path: `/tmp/${body.name}` };
        state.repos.push(r);
        return jsonResp(r, 201);
      }
      return new Response("", { status: 404 });
    }),
  );
  return state;
}

describe("Sidebar", () => {
  beforeEach(() => {
    vi.stubGlobal(
      "confirm",
      vi.fn(() => true),
    );
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function setup() {
    return render(
      <SessionProvider>
        <Sidebar />
      </SessionProvider>,
    );
  }

  it("renders repo groups and their sessions", async () => {
    const state = installFetchMock();
    state.repos.push({ name: "alpha", path: "/tmp/alpha" });
    state.repos.push({ name: "beta", path: "/tmp/beta" });
    state.sessions.push({
      id: "11111111-1111-1111-1111-111111111111",
      repo: "alpha",
      working_dir: "/tmp/alpha",
      state: "live",
      created_at: new Date().toISOString(),
      ended_at: null,
      exit_code: null,
      current_claude_session_uuid: null,
    });
    setup();
    await waitFor(() => {
      expect(screen.getByText("alpha")).toBeDefined();
      expect(screen.getByText("beta")).toBeDefined();
    });
    await waitFor(() => {
      expect(screen.getByText(/11111111/)).toBeDefined();
    });
  });

  it("new-session inline form creates a session via the API", async () => {
    const state = installFetchMock();
    state.repos.push({ name: "alpha", path: "/tmp/alpha" });
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText("alpha")).toBeDefined());

    // Find the "+" button scoped to the alpha group.
    const spawnBtn = screen.getByLabelText("New session in alpha");
    await user.click(spawnBtn);
    const submitBtn = screen.getByRole("button", { name: /spawn/i });
    await user.click(submitBtn);

    await waitFor(() => {
      expect(state.createSessionCalls.length).toBe(1);
    });
    expect(state.createSessionCalls[0]).toEqual({ repo: "alpha" });
  });

  it("delete button removes the session via DELETE", async () => {
    const state = installFetchMock();
    state.repos.push({ name: "alpha", path: "/tmp/alpha" });
    state.sessions.push({
      id: "22222222-2222-2222-2222-222222222222",
      repo: "alpha",
      working_dir: "/tmp/alpha",
      state: "live",
      created_at: new Date().toISOString(),
      ended_at: null,
      exit_code: null,
      current_claude_session_uuid: null,
    });
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(/22222222/)).toBeDefined());

    const deleteButtons = screen.getAllByLabelText("Delete session");
    await user.click(deleteButtons[0]);

    await waitFor(() => {
      expect(state.deletedIds.length).toBe(1);
    });
  });

  it("new-repo form POSTs to /api/repos", async () => {
    const state = installFetchMock();
    setup();
    const user = userEvent.setup();

    await waitFor(() => expect(screen.getByText(/no repos yet/i)).toBeDefined());

    const plus = screen.getByLabelText("New repo");
    await user.click(plus);

    const nameInput = screen.getByPlaceholderText(/repo name/i);
    await user.type(nameInput, "newrepo");
    const createBtn = screen.getByRole("button", { name: /create/i });
    await user.click(createBtn);

    await waitFor(() => {
      expect(state.createRepoCalls.length).toBe(1);
    });
    expect(state.createRepoCalls[0]).toEqual({
      name: "newrepo",
      git_url: undefined,
    });
  });
});
