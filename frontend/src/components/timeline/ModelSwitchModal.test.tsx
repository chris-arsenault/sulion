import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { ModelSwitchBanner, ModelSwitchModal } from "./ModelSwitchModal";
import type { ModelSwitchView, SessionView } from "../../api/types";

const sessionId = "11111111-1111-1111-1111-111111111111";

function noop() {}
async function asyncNoop() {}

const session: SessionView = {
  id: sessionId,
  repo: "sigillum-explorations",
  working_dir: "/home/sulion/repos/sigillum-explorations",
  state: "live",
  created_at: "2026-09-17T07:00:00Z",
  ended_at: null,
  exit_code: null,
  current_session_uuid: "01a0ae35-7632-7fb2-967f-989c6da175d0",
  current_session_agent: "codex",
  last_event_at: null,
  label: "pocket",
  pinned: false,
  color: null,
  agent_runtime: {
    agent: "codex",
    state: "running",
    started_at: "2026-09-17T07:12:00Z",
    ended_at: null,
    exit_code: null,
  },
  future_prompts_pending_count: 0,
};

function codexSwitch(overrides: Partial<ModelSwitchView> = {}): ModelSwitchView {
  return {
    id: "sw-1",
    agent: "codex",
    source: "codex_thread_settings",
    from_model: "gpt-6-astra",
    to_model: "gpt-5.6-luna",
    from_effort: "high",
    to_effort: "medium",
    turn_id: null,
    turn_in_flight: false,
    context: {
      service_tier: "default",
      rate_limits: {
        limit_id: "codex",
        plan_type: "pro",
        rate_limit_reached_type: null,
        primary: { used_percent: 92, window_minutes: 10080, resets_at: 1789865705 },
        secondary: null,
        credits: { has_credits: false, unlimited: false, balance: "0" },
      },
    },
    observed_at: "2026-09-17T07:50:36Z",
    interrupted_at: null,
    interrupt_error: null,
    ...overrides,
  };
}

function claudeSwitch(): ModelSwitchView {
  return {
    id: "sw-2",
    agent: "claude-code",
    source: "claude_fallback",
    from_model: "claude-fable-5",
    to_model: "claude-opus-4-8",
    from_effort: "high",
    to_effort: "high",
    turn_id: null,
    turn_in_flight: true,
    context: {
      fallback: { from: "claude-fable-5", to: "claude-opus-4-8" },
      iterations: [
        { type: "message", model: "claude-fable-5" },
        { type: "fallback_message", model: "claude-opus-4-8" },
      ],
      service_tier: "standard",
    },
    observed_at: "2026-09-02T01:00:00Z",
    interrupted_at: "2026-09-02T01:00:04Z",
    interrupt_error: null,
  };
}

function stubFetch() {
  const requests: Array<{ url: string; method: string; body: unknown }> = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      requests.push({
        url: String(input),
        method: init?.method ?? "GET",
        body: init?.body ? JSON.parse(init.body as string) : null,
      });
      return new Response(null, { status: 204 });
    }),
  );
  return requests;
}

function text(element: HTMLElement): string {
  return element.textContent ?? "";
}

describe("ModelSwitchModal", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("shows the Codex switch with its rate-limit context and confirms with adopt", async () => {
    const requests = stubFetch();
    const onAcknowledged = vi.fn(asyncNoop);
    render(
      <ModelSwitchModal
        open
        sessionId={sessionId}
        session={session}
        modelSwitch={codexSwitch()}
        onHide={noop}
        onAcknowledged={onAcknowledged}
      />,
    );

    const headline = text(screen.getByTestId("model-switch-headline"));
    expect(headline).toContain("gpt-6-astra");
    expect(headline).toContain("gpt-5.6-luna");
    expect(screen.getByText(/high → medium/)).toBeTruthy();
    expect(screen.getByText(/Codex thread settings/)).toBeTruthy();
    expect(
      screen.getByText(
        /The change happened between turns\. The next turn on gpt-5\.6-luna will be stopped/,
      ),
    ).toBeTruthy();
    const limits = text(screen.getByTestId("model-switch-rate-limits"));
    expect(limits).toContain("92% of the 7-day window used");
    expect(limits).toContain("pro");
    expect(limits).toContain("none");

    await userEvent.click(screen.getByRole("button", { name: "Continue with gpt-5.6-luna" }));
    await waitFor(() => expect(onAcknowledged).toHaveBeenCalledTimes(1));
    expect(requests).toEqual([
      {
        url: `/api/sessions/${sessionId}/model-switches/sw-1/acknowledge`,
        method: "POST",
        body: { adopt: true },
      },
    ]);
  });

  it("dismisses without adopting the new model", async () => {
    const requests = stubFetch();
    const onAcknowledged = vi.fn(asyncNoop);
    render(
      <ModelSwitchModal
        open
        sessionId={sessionId}
        session={session}
        modelSwitch={codexSwitch({ interrupt_error: "development node is unavailable" })}
        onHide={noop}
        onAcknowledged={onAcknowledged}
      />,
    );
    expect(screen.getByText(/Interrupt failed: development node is unavailable/)).toBeTruthy();

    await userEvent.click(
      screen.getByRole("button", { name: "Dismiss, I'll switch back to gpt-6-astra" }),
    );
    await waitFor(() => expect(onAcknowledged).toHaveBeenCalledTimes(1));
    expect(requests[0].body).toEqual({ adopt: false });
  });

  it("explains a Claude fallback and reports the interrupted turn", () => {
    stubFetch();
    render(
      <ModelSwitchModal
        open
        sessionId={sessionId}
        session={session}
        modelSwitch={claudeSwitch()}
        onHide={noop}
        onAcknowledged={asyncNoop}
      />,
    );
    expect(screen.getByText(/Claude Code · pocket/)).toBeTruthy();
    expect(
      screen.getByText(/Claude Code fell back from claude-fable-5 to claude-opus-4-8/),
    ).toBeTruthy();
    expect(screen.getByText("fallback_message")).toBeTruthy();
    expect(
      screen.getByText(/The turn running on claude-opus-4-8 was interrupted at/),
    ).toBeTruthy();
  });

  it("surfaces an acknowledgement failure and stays open", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(JSON.stringify({ error: "switch not found" }), {
            status: 404,
            headers: { "content-type": "application/json" },
          }),
      ),
    );
    const onAcknowledged = vi.fn(asyncNoop);
    render(
      <ModelSwitchModal
        open
        sessionId={sessionId}
        session={session}
        modelSwitch={codexSwitch()}
        onHide={noop}
        onAcknowledged={onAcknowledged}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Continue with gpt-5.6-luna" }));
    await waitFor(() => expect(screen.getByText(/switch not found/)).toBeTruthy());
    expect(onAcknowledged).not.toHaveBeenCalled();
    expect(screen.getByTestId("model-switch-modal")).toBeTruthy();
  });

  it("hides on Escape and the banner reopens it", async () => {
    stubFetch();
    const onHide = vi.fn();
    render(
      <ModelSwitchModal
        open
        sessionId={sessionId}
        session={session}
        modelSwitch={codexSwitch()}
        onHide={onHide}
        onAcknowledged={asyncNoop}
      />,
    );
    await userEvent.keyboard("{Escape}");
    expect(onHide).toHaveBeenCalledTimes(1);

    const onReview = vi.fn();
    render(<ModelSwitchBanner modelSwitch={codexSwitch()} onReview={onReview} />);
    const banner = text(screen.getByTestId("model-switch-banner"));
    expect(banner).toContain("Model changed to gpt-5.6-luna");
    expect(banner).toContain("was gpt-6-astra");
    await userEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(onReview).toHaveBeenCalledTimes(1);
  });
});
