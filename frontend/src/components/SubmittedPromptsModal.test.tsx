import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { SubmittedPromptsModal } from "./SubmittedPromptsModal";
import type { SessionView, SubmittedPromptListResponse } from "../api/types";
import { useSessionStore } from "../state/SessionStore";
import { appStatePayload, jsonResponse } from "../test/appState";

const sessionId = "11111111-1111-1111-1111-111111111111";

function noop() {}

const runningSession: SessionView = {
  id: sessionId,
  repo: "demo",
  working_dir: "/home/sulion/repos/demo",
  state: "live",
  created_at: "2026-09-12T08:00:00Z",
  ended_at: null,
  exit_code: null,
  current_session_uuid: null,
  current_session_agent: null,
  last_event_at: null,
  label: null,
  pinned: false,
  color: null,
  agent_runtime: {
    agent: "claude",
    state: "running",
    started_at: "2026-09-12T09:00:00Z",
    ended_at: null,
    exit_code: null,
  },
  future_prompts_pending_count: 0,
};

function listing(): SubmittedPromptListResponse {
  return {
    gate: "starting",
    prompts: [
      {
        id: "p1",
        agent: "claude",
        text: "refactor the ingester",
        forced: false,
        submitted_at: "2026-09-12T10:00:00Z",
        state: "unmatched",
        delivery_error: null,
        matched_turn_key: null,
        matched_at: null,
        dismissed_at: null,
      },
      {
        id: "p2",
        agent: "claude",
        text: "earlier prompt",
        forced: false,
        submitted_at: "2026-09-12T09:00:00Z",
        state: "matched",
        delivery_error: null,
        matched_turn_key: "abc:100",
        matched_at: "2026-09-12T09:00:03Z",
        dismissed_at: null,
      },
    ],
  };
}

describe("SubmittedPromptsModal", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("lists submitted prompts, retries with force, and dismisses", async () => {
    const payload = appStatePayload({ sessions: [runningSession] });
    useSessionStore.setState({ sessions: [runningSession] });

    const requests: Array<{ url: string; method: string; body: unknown }> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input);
        const method = init?.method ?? "GET";
        requests.push({
          url,
          method,
          body: init?.body ? JSON.parse(init.body as string) : null,
        });
        if (url.endsWith("/submitted-prompts") && method === "GET") {
          return jsonResponse(listing());
        }
        if (url.endsWith("/prompt") && method === "POST") {
          return new Response("", { status: 202 });
        }
        if (url.includes("/submitted-prompts/") && method === "DELETE") {
          return new Response("", { status: 204 });
        }
        return jsonResponse(payload);
      }),
    );

    render(<SubmittedPromptsModal open sessionId={sessionId} onClose={noop} />);
    expect(await screen.findByText("refactor the ingester")).toBeDefined();
    expect(screen.getByText("earlier prompt")).toBeDefined();
    expect(screen.getByRole("status").textContent).toContain("still starting");

    const user = userEvent.setup();
    const retryButtons = screen.getAllByRole("button", { name: "Retry" });
    await user.click(retryButtons[0]);
    await waitFor(() =>
      expect(
        requests.find((r) => r.method === "POST" && r.url.endsWith("/prompt"))?.body,
      ).toEqual({ text: "refactor the ingester", force: true }),
    );

    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    await waitFor(() =>
      expect(
        requests.some(
          (r) => r.method === "DELETE" && r.url.endsWith("/submitted-prompts/p1"),
        ),
      ).toBe(true),
    );
  });
});
