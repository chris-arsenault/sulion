import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { Turn, ToolPair } from "./grouping";
import type { TimelineEvent } from "../../api/types";
import { TurnDetail } from "./TurnDetail";

function userEv(text: string, byte = 100): TimelineEvent {
  return {
    byte_offset: byte,
    timestamp: "2025-01-01T00:00:00Z",
    kind: "user",
    payload: {
      type: "user",
      message: { role: "user", content: [{ type: "text", text }] },
    },
  };
}

function assistantEv(blocks: unknown[], byte = 200): TimelineEvent {
  return {
    byte_offset: byte,
    timestamp: "2025-01-01T00:00:02Z",
    kind: "assistant",
    payload: {
      type: "assistant",
      message: { role: "assistant", content: blocks },
    },
  };
}

function mkTurn(prompt: TimelineEvent, events: TimelineEvent[], overrides: Partial<Turn> = {}): Turn {
  return {
    id: prompt.byte_offset,
    userPrompt: prompt,
    events: [prompt, ...events],
    startTimestamp: prompt.timestamp,
    endTimestamp: events[events.length - 1]?.timestamp ?? prompt.timestamp,
    durationMs: 2000,
    toolPairs: [],
    thinkingCount: 0,
    hasErrors: false,
    ...overrides,
  };
}

describe("TurnDetail", () => {
  it("renders the user prompt in the sticky header", () => {
    const prompt = userEv("inspect me");
    const turn = mkTurn(prompt, [assistantEv([{ type: "text", text: "ok" }])]);
    render(<TurnDetail turn={turn} showThinking={true} />);
    expect(screen.getByText("inspect me")).toBeDefined();
  });

  it("renders assistant text", () => {
    const prompt = userEv("p");
    const turn = mkTurn(prompt, [
      assistantEv([{ type: "text", text: "here is the reply" }]),
    ]);
    render(<TurnDetail turn={turn} showThinking={true} />);
    expect(screen.getByText("here is the reply")).toBeDefined();
  });

  it("renders thinking as a chip that opens a fly-out; fly-out shows thinking text", async () => {
    const prompt = userEv("p");
    const turn = mkTurn(
      prompt,
      [
        assistantEv([
          { type: "thinking", thinking: "private reasoning here" },
          { type: "text", text: "public reply" },
        ]),
      ],
      { thinkingCount: 1 },
    );
    const user = userEvent.setup();
    render(<TurnDetail turn={turn} showThinking={true} />);

    // Chip is visible by default; full thinking content is only in the flyout
    expect(screen.queryByText("private reasoning here")).toBeNull();
    const chip = screen.getByRole("button", { name: /💭 thinking/i });
    await user.click(chip);
    expect(screen.getByTestId("thinking-flyout")).toBeDefined();
    expect(screen.getByText("private reasoning here")).toBeDefined();
  });

  it("hides thinking chip when showThinking=false", () => {
    const prompt = userEv("p");
    const turn = mkTurn(
      prompt,
      [
        assistantEv([
          { type: "thinking", thinking: "hidden" },
          { type: "text", text: "visible" },
        ]),
      ],
      { thinkingCount: 1 },
    );
    render(<TurnDetail turn={turn} showThinking={false} />);
    expect(screen.queryByText(/💭 thinking/i)).toBeNull();
    expect(screen.getByText("visible")).toBeDefined();
  });

  it("renders a successful tool pair collapsed by default (low-signal), with 'ok' status", () => {
    const prompt = userEv("p");
    const use = { type: "tool_use", id: "t1", name: "Read", input: { file_path: "/a" } };
    const asst = assistantEv([use]);
    const pair: ToolPair = {
      id: "t1",
      name: "Read",
      input: { file_path: "/a" },
      use: use as never,
      useEvent: asst,
      result: { type: "tool_result", tool_use_id: "t1", content: "file body" },
      resultEvent: asst,
      isError: false,
      isPending: false,
    };
    const turn = mkTurn(prompt, [asst], { toolPairs: [pair] });
    render(<TurnDetail turn={turn} showThinking={true} />);
    // Collapsed state shows an ok chip but hides the result body
    expect(screen.getByText(/ok/i)).toBeDefined();
    expect(screen.queryByText("file body")).toBeNull();
  });

  it("expands errored tool pair by default and shows the error body", () => {
    const prompt = userEv("p");
    const use = { type: "tool_use", id: "e1", name: "Bash", input: { command: "oops" } };
    const asst = assistantEv([use]);
    const pair: ToolPair = {
      id: "e1",
      name: "Bash",
      input: { command: "oops" },
      use: use as never,
      useEvent: asst,
      result: {
        type: "tool_result",
        tool_use_id: "e1",
        content: "command failed",
        is_error: true,
      },
      resultEvent: asst,
      isError: true,
      isPending: false,
    };
    const turn = mkTurn(prompt, [asst], { toolPairs: [pair], hasErrors: true });
    render(<TurnDetail turn={turn} showThinking={true} />);
    expect(screen.getByText(/command failed/)).toBeDefined();
  });

  it("Task tool exposes View agent log button that fires onOpenSubagent", async () => {
    const prompt = userEv("spawn");
    const use = {
      type: "tool_use",
      id: "t1",
      name: "Task",
      input: { subagent_type: "Explore", prompt: "find stuff" },
    };
    const asst = assistantEv([use]);
    const pair: ToolPair = {
      id: "t1",
      name: "Task",
      input: use.input,
      use: use as never,
      useEvent: asst,
      result: null,
      resultEvent: null,
      isError: false,
      isPending: true,
    };
    const turn = mkTurn(prompt, [asst], { toolPairs: [pair] });
    const onOpen = vi.fn();
    const user = userEvent.setup();
    render(
      <TurnDetail turn={turn} showThinking={true} onOpenSubagent={onOpen} />,
    );
    await user.click(
      screen.getByRole("button", { name: /view agent log/i }),
    );
    expect(onOpen).toHaveBeenCalledWith(pair);
  });
});
