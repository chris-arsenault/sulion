import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { FuturePromptsModal } from "./FuturePromptsModal";
import { resetSessionStore, useSessionStore } from "../state/SessionStore";
import { subscribeToAppCommands } from "../state/AppCommands";

const SESSION_ID = "11111111-1111-1111-1111-111111111111";
const SESSION_UUID = "22222222-2222-2222-2222-222222222222";

function deferred<T>() {
  let resolveFn: ((value: T) => void) | null = null;
  return {
    promise: new Promise<T>((resolve) => {
      resolveFn = resolve;
    }),
    resolve(value: T) {
      if (!resolveFn) throw new Error("deferred promise was not initialized");
      resolveFn(value);
    },
  };
}

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
          future_prompts_pending_count: 0,
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
    const createResponse = deferred<Response>();
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
        return createResponse.promise;
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
    const composer = screen.getByPlaceholderText(/queue the next thing/i) as HTMLTextAreaElement;
    await user.type(composer, "ask about tests later");
    await user.click(screen.getByRole("button", { name: "Add" }));

    const createdPrompt = screen.findByText("ask about tests later");
    const draftCleared = waitFor(() => expect(composer.value).toBe(""));
    const composerEnabled = waitFor(() => expect(composer.disabled).toBe(false));

    createResponse.resolve(
      new Response(JSON.stringify({
        id: "fp-1",
        state: "pending",
        created_at: "2026-04-20T01:00:00Z",
        updated_at: "2026-04-20T01:00:00Z",
        text: "ask about tests later",
      }), {
        status: 201,
        headers: { "Content-Type": "application/json" },
      }),
    );

    expect(await createdPrompt).toBeDefined();
    await draftCleared;
    await composerEnabled;
  });

  it("injects a pending prompt and marks it sent", async () => {
    seedSession();
    const seen: Array<unknown> = [];
    const unsubscribe = subscribeToAppCommands((command) => {
      seen.push(command);
    });

    const patchResponse = deferred<Response>();
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
        return patchResponse.promise;
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

    const injected = waitFor(() =>
      expect(seen).toContainEqual({
        type: "inject-terminal",
        sessionId: SESSION_ID,
        text: "follow up after this run",
      }),
    );
    const sendGone = waitFor(() =>
      expect(screen.queryByRole("button", { name: "Send" })).toBeNull(),
    );

    patchResponse.resolve(
      new Response(JSON.stringify({
        id: "fp-1",
        state: "sent",
        created_at: "2026-04-20T01:00:00Z",
        updated_at: "2026-04-20T01:05:00Z",
        text: "follow up after this run",
      }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );

    await injected;
    await sendGone;
    unsubscribe();
  });
});
