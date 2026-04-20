import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { FuturePromptsModal } from "./FuturePromptsModal";
import { resetSessionStore, useSessionStore } from "../state/SessionStore";
import { subscribeToAppCommands } from "../state/AppCommands";

const SESSION_ID = "11111111-1111-1111-1111-111111111111";
const SESSION_UUID = "22222222-2222-2222-2222-222222222222";

describe("FuturePromptsModal", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    resetSessionStore();
  });

  function seedSession() {
    useSessionStore.setState({
      sessions: [
        {
          id: SESSION_ID,
          repo: "alpha",
          working_dir: "/tmp/alpha",
          state: "live",
          created_at: "2026-04-20T00:00:00Z",
          ended_at: null,
          exit_code: null,
          current_session_uuid: SESSION_UUID,
          current_session_agent: "codex",
          last_event_at: null,
          label: "main",
          pinned: false,
          color: null,
        },
      ],
      repos: [],
      selectedSessionId: null,
      lastError: null,
      sessionsLoaded: true,
    });
  }

  it("creates a new pending future prompt", async () => {
    seedSession();
    const fetchMock = vi.fn(async (input: RequestInfo, init?: RequestInit) => {
      const url = typeof input === "string" ? input : input.url;
      const method = init?.method ?? "GET";
      if (url === "/api/sessions" && method === "GET") {
        return new Response(JSON.stringify({ sessions: useSessionStore.getState().sessions }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      if (url === "/api/repos" && method === "GET") {
        return new Response(JSON.stringify({ repos: [] }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      if (url === `/api/sessions/${SESSION_ID}/future-prompts` && method === "GET") {
        return new Response(JSON.stringify({
          session_uuid: SESSION_UUID,
          session_agent: "codex",
          prompts: [],
        }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      if (url === `/api/sessions/${SESSION_ID}/future-prompts` && method === "PUT") {
        expect(JSON.parse(init?.body as string)).toEqual({ text: "ask about tests later" });
        return new Response(JSON.stringify({
          id: "fp-1",
          state: "pending",
          created_at: "2026-04-20T01:00:00Z",
          updated_at: "2026-04-20T01:00:00Z",
          text: "ask about tests later",
        }), {
          status: 201,
          headers: { "Content-Type": "application/json" },
        });
      }
      return new Response("", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    render(
      <FuturePromptsModal open sessionId={SESSION_ID} onClose={() => {}} />,
    );

    const user = userEvent.setup();
    await waitFor(() =>
      expect(screen.getByPlaceholderText(/queue the next thing/i)).toBeDefined(),
    );
    await user.type(screen.getByPlaceholderText(/queue the next thing/i), "ask about tests later");
    await user.click(screen.getByRole("button", { name: "Add" }));

    await waitFor(() =>
      expect(screen.getByText("ask about tests later")).toBeDefined(),
    );
  });

  it("injects a pending prompt and marks it sent", async () => {
    seedSession();
    const seen: Array<unknown> = [];
    const unsubscribe = subscribeToAppCommands((command) => {
      seen.push(command);
    });

    const fetchMock = vi.fn(async (input: RequestInfo, init?: RequestInit) => {
      const url = typeof input === "string" ? input : input.url;
      const method = init?.method ?? "GET";
      if (url === "/api/sessions" && method === "GET") {
        return new Response(JSON.stringify({ sessions: useSessionStore.getState().sessions }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      if (url === "/api/repos" && method === "GET") {
        return new Response(JSON.stringify({ repos: [] }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      if (url === `/api/sessions/${SESSION_ID}/future-prompts` && method === "GET") {
        return new Response(JSON.stringify({
          session_uuid: SESSION_UUID,
          session_agent: "codex",
          prompts: [
            {
              id: "fp-1",
              state: "pending",
              created_at: "2026-04-20T01:00:00Z",
              updated_at: "2026-04-20T01:00:00Z",
              text: "follow up after this run",
            },
          ],
        }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      if (url === `/api/sessions/${SESSION_ID}/future-prompts/fp-1` && method === "PATCH") {
        expect(JSON.parse(init?.body as string)).toEqual({ state: "sent" });
        return new Response(JSON.stringify({
          id: "fp-1",
          state: "sent",
          created_at: "2026-04-20T01:00:00Z",
          updated_at: "2026-04-20T01:05:00Z",
          text: "follow up after this run",
        }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      return new Response("", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    render(
      <FuturePromptsModal open sessionId={SESSION_ID} onClose={() => {}} />,
    );

    const user = userEvent.setup();
    await waitFor(() =>
      expect(screen.getByText("follow up after this run")).toBeDefined(),
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() =>
      expect(seen).toContainEqual({
        type: "inject-terminal",
        sessionId: SESSION_ID,
        text: "follow up after this run",
      }),
    );
    unsubscribe();
  });
});
