import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { SessionEndedPane } from "./SessionEndedPane";

const orphanedSession = {
  id: "11111111-1111-1111-1111-111111111111",
  repo: "ahara",
  working_dir: "/home/dev/repos/ahara",
  state: "orphaned" as const,
  created_at: new Date(Date.now() - 3_600_000).toISOString(),
  ended_at: new Date().toISOString(),
  exit_code: null,
  current_session_uuid: "deadbeef-dead-beef-dead-beefdeadbeef",
  current_session_agent: "claude-code",
  last_event_at: null,
  label: null,
  pinned: false,
  color: null,
};

const deadSession = {
  ...orphanedSession,
  id: "22222222-2222-2222-2222-222222222222",
  state: "dead" as const,
  exit_code: 0,
  current_session_uuid: null,
  current_session_agent: null,
};

interface FetchState {
  createSessionCalls: unknown[];
  deletedIds: string[];
}

function installFetchMock(state: FetchState) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo, init?: RequestInit) => {
      const url = typeof input === "string" ? input : (input as Request).url;
      const method = init?.method ?? "GET";
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        });

      if (url === "/api/sessions" && method === "GET") return json({ sessions: [] });
      if (url === "/api/repos" && method === "GET") return json({ repos: [] });
      if (url === "/api/sessions" && method === "POST") {
        state.createSessionCalls.push(JSON.parse(init!.body as string));
        return json(
          {
            id: "99999999-9999-9999-9999-999999999999",
            repo: "ahara",
            working_dir: "/home/dev/repos/ahara",
            state: "live",
            created_at: new Date().toISOString(),
            ended_at: null,
            exit_code: null,
            current_session_uuid: null,
            current_session_agent: null,
          },
          201,
        );
      }
      if (url.startsWith("/api/sessions/") && method === "DELETE") {
        const id = url.split("/").pop()!;
        state.deletedIds.push(id);
        return new Response(null, { status: 204 });
      }
      return new Response("", { status: 404 });
    }),
  );
}

describe("SessionEndedPane", () => {
  let state: FetchState;
  beforeEach(() => {
    state = { createSessionCalls: [], deletedIds: [] };
    installFetchMock(state);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function setup(session: typeof orphanedSession | typeof deadSession) {
    return render(<SessionEndedPane session={session} />);
  }

  it("renders the orphaned badge and explanation", () => {
    setup(orphanedSession);
    expect(screen.getByText("Session orphaned")).toBeDefined();
    expect(
      screen.getByText(/backend restarted/i),
    ).toBeDefined();
    expect(screen.getByText("ahara")).toBeDefined();
  });

  it("renders the dead badge with exit code when present", () => {
    setup(deadSession);
    expect(screen.getByText("Session ended")).toBeDefined();
    expect(screen.getByText(/exit/i)).toBeDefined();
  });

  it("shows Resume for orphaned Claude sessions and posts generic resume fields", async () => {
    setup(orphanedSession);
    const user = userEvent.setup();
    const resumeBtn = screen.getByRole("button", { name: /resume/i });
    await user.click(resumeBtn);
    await waitFor(() => {
      expect(state.createSessionCalls.length).toBe(1);
    });
    expect(state.createSessionCalls[0]).toMatchObject({
      repo: "ahara",
      working_dir: "/home/dev/repos/ahara",
      resume_session_uuid: "deadbeef-dead-beef-dead-beefdeadbeef",
      resume_agent: "claude-code",
    });
  });

  it("does not show Resume on dead sessions with no correlated session", () => {
    setup(deadSession);
    expect(screen.queryByRole("button", { name: /resume/i })).toBeNull();
    expect(screen.getByRole("button", { name: /delete/i })).toBeDefined();
  });

  it("shows Resume for orphaned Codex sessions and posts the Codex agent", async () => {
    setup({
      ...orphanedSession,
      current_session_agent: "codex",
    });
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /resume/i }));
    await waitFor(() => {
      expect(state.createSessionCalls.length).toBe(1);
    });
    expect(state.createSessionCalls[0]).toMatchObject({
      resume_session_uuid: "deadbeef-dead-beef-dead-beefdeadbeef",
      resume_agent: "codex",
    });
  });

  it("clicking Delete fires DELETE against the session id", async () => {
    setup(orphanedSession);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /delete/i }));
    await waitFor(() => {
      expect(state.deletedIds).toEqual([orphanedSession.id]);
    });
  });
});
