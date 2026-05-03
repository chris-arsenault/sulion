import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { SessionView } from "../api/types";
import { SessionEndedPane } from "./SessionEndedPane";
import { resetSessionStore, useSessionStore } from "../state/SessionStore";
import { resetTabStore, useTabStore } from "../state/TabStore";
import { appStatePayload, jsonResponse } from "../test/appState";

const resumeSessionUuid = "deadbeef-dead-beef-dead-beefdeadbeef";
const successorSessionId = "99999999-9999-9999-9999-999999999999";
const workspaceSessionId = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

const orphanedSession: SessionView = {
  id: "11111111-1111-1111-1111-111111111111",
  repo: "ahara",
  working_dir: "/home/dev/repos/ahara",
  state: "orphaned",
  created_at: new Date(Date.now() - 3_600_000).toISOString(),
  ended_at: new Date().toISOString(),
  exit_code: null,
  current_session_uuid: resumeSessionUuid,
  current_session_agent: "claude-code",
  last_event_at: null,
  label: null,
  pinned: false,
  color: null,
  future_prompts_pending_count: 0,
};

const deadSession: SessionView = {
  ...orphanedSession,
  id: "22222222-2222-2222-2222-222222222222",
  state: "dead",
  exit_code: 0,
  current_session_uuid: null,
  current_session_agent: null,
};

interface FetchState {
  createSessionCalls: unknown[];
  deletedIds: string[];
  patches: Array<{ id: string; body: unknown }>;
}

function installFetchMock(state: FetchState) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo, init?: RequestInit) => {
      const url = typeof input === "string" ? input : (input as Request).url;
      const method = init?.method ?? "GET";
      const json = jsonResponse;

      if (url === "/api/app-state" && method === "GET") {
        return json(appStatePayload());
      }
      if (url === "/api/sessions" && method === "POST") {
        state.createSessionCalls.push(JSON.parse(init!.body as string));
        return json(
          {
            id: successorSessionId,
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
      if (url.startsWith("/api/sessions/") && method === "PATCH") {
        const id = url.split("/").pop()!;
        const body = JSON.parse(init!.body as string);
        state.patches.push({ id, body });
        return new Response(null, { status: 204 });
      }
      return new Response("", { status: 404 });
    }),
  );
}

describe("SessionEndedPane", () => {
  let state: FetchState;
  beforeEach(() => {
    state = { createSessionCalls: [], deletedIds: [], patches: [] };
    resetSessionStore();
    resetTabStore();
    installFetchMock(state);
  });
  afterEach(() => {
    resetSessionStore();
    resetTabStore();
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

  it("resumes an orphaned session by rebinding tabs and deleting the old row", async () => {
    useTabStore.getState().openTab(
      { kind: "terminal", sessionId: orphanedSession.id },
      "top",
    );
    useTabStore.getState().openTab(
      { kind: "timeline", sessionId: orphanedSession.id },
      "bottom",
    );
    useSessionStore.getState().selectSession(orphanedSession.id);
    setup(orphanedSession);
    const user = userEvent.setup();
    const resumeBtn = screen.getByRole("button", { name: /resume/i });
    await user.click(resumeBtn);
    await waitFor(() => {
      expect(state.createSessionCalls.length).toBe(1);
      expect(state.deletedIds).toEqual([orphanedSession.id]);
    });
    expect(state.createSessionCalls[0]).toMatchObject({
      repo: "ahara",
      working_dir: "/home/dev/repos/ahara",
      workspace_mode: "main",
      resume_session_uuid: resumeSessionUuid,
      resume_agent: "claude-code",
    });
    expect(useSessionStore.getState().selectedSessionId).toBe(
      successorSessionId,
    );
    const tabs = Object.values(useTabStore.getState().tabs);
    expect(
      tabs.find((tab) => tab.kind === "terminal")?.sessionId,
    ).toBe(successorSessionId);
    expect(
      tabs.find((tab) => tab.kind === "timeline")?.sessionId,
    ).toBe(successorSessionId);
  });

  it("carries the orphan's label, pin, and colour onto the successor session", async () => {
    const customised = {
      ...orphanedSession,
      label: "migration branch",
      pinned: true,
      color: "amber" as const,
    };
    useSessionStore.getState().selectSession(customised.id);
    setup(customised);

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /resume/i }));

    await waitFor(() => {
      expect(state.createSessionCalls.length).toBe(1);
      expect(state.patches.length).toBe(1);
    });
    expect(state.patches[0]).toEqual({
      id: successorSessionId,
      body: {
        label: "migration branch",
        pinned: true,
        color: "amber",
      },
    });
  });

  it("skips the customisation patch when the orphan has no label / pin / colour", async () => {
    useSessionStore.getState().selectSession(orphanedSession.id);
    setup(orphanedSession);

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /resume/i }));

    await waitFor(() => {
      expect(state.createSessionCalls.length).toBe(1);
    });
    expect(state.patches.length).toBe(0);
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
      resume_session_uuid: resumeSessionUuid,
      resume_agent: "codex",
    });
  });

  it("resumes workspace-bound orphaned sessions in the same workspace", async () => {
    setup({
      ...orphanedSession,
      working_dir: "/home/dev/workspaces/ahara/ws-1",
      workspace: {
        id: workspaceSessionId,
        repo_name: "ahara",
        kind: "worktree",
        path: "/home/dev/workspaces/ahara/ws-1",
        branch_name: "sulion/ahara/ws-1",
        base_ref: "main",
        base_sha: "abc123",
        merge_target: "main",
      },
    });
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /resume/i }));
    await waitFor(() => {
      expect(state.createSessionCalls.length).toBe(1);
    });
    expect(state.createSessionCalls[0]).toMatchObject({
      repo: "ahara",
      workspace_id: workspaceSessionId,
      resume_session_uuid: resumeSessionUuid,
      resume_agent: "claude-code",
    });
    expect(state.createSessionCalls[0]).not.toHaveProperty("working_dir");
    expect(state.createSessionCalls[0]).not.toHaveProperty("workspace_mode");
  });

  it("resumes main-workspace orphaned sessions with their stored working dir", async () => {
    setup({
      ...orphanedSession,
      working_dir: "/home/dev/repos/ahara/packages/api",
      workspace: {
        id: workspaceSessionId,
        repo_name: "ahara",
        kind: "main",
        path: "/home/dev/repos/ahara",
        branch_name: "main",
        base_ref: "main",
        base_sha: "abc123",
        merge_target: "main",
      },
    });
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /resume/i }));
    await waitFor(() => {
      expect(state.createSessionCalls.length).toBe(1);
    });
    expect(state.createSessionCalls[0]).toMatchObject({
      repo: "ahara",
      working_dir: "/home/dev/repos/ahara/packages/api",
      workspace_mode: "main",
      resume_session_uuid: resumeSessionUuid,
      resume_agent: "claude-code",
    });
    expect(state.createSessionCalls[0]).not.toHaveProperty("workspace_id");
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
