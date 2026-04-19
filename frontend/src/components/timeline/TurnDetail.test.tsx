import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { TimelineEvent } from "../../api/types";
import type { Turn, ToolPair } from "./grouping";
import { TurnDetail } from "./TurnDetail";
import { makeEvent, textBlock, thinkingBlock, toolUseBlock } from "./test-helpers";

function userEv(text: string, byte = 100) {
  return makeEvent("user", {
    byte_offset: byte,
    timestamp: "2025-01-01T00:00:00Z",
    blocks: [textBlock(0, text)],
  });
}

function assistantEv(blocks: ReturnType<typeof textBlock>[], byte = 200) {
  return makeEvent("assistant", {
    byte_offset: byte,
    timestamp: "2025-01-01T00:00:02Z",
    blocks,
  });
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
    const turn = mkTurn(prompt, [assistantEv([textBlock(0, "ok")])]);
    render(<TurnDetail turn={turn} showThinking={true} />);
    expect(screen.getByText("inspect me")).toBeDefined();
  });

  it("renders assistant text", () => {
    const prompt = userEv("p");
    const turn = mkTurn(prompt, [
      assistantEv([textBlock(0, "here is the reply")]),
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
          thinkingBlock(0, "private reasoning here"),
          textBlock(1, "public reply"),
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
          thinkingBlock(0, "hidden"),
          textBlock(1, "visible"),
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
    const useBlock = toolUseBlock(0, "t1", "read", { path: "/a" }, "Read");
    const use = { type: "tool_use", id: "t1", name: "read", input: { path: "/a" } };
    const asst = assistantEv([useBlock]);
    const pair: ToolPair = {
      id: "t1",
      name: "read",
      input: { path: "/a" },
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
    const useBlock = toolUseBlock(0, "e1", "bash", { command: "oops" }, "Bash");
    const use = { type: "tool_use", id: "e1", name: "bash", input: { command: "oops" } };
    const asst = assistantEv([useBlock]);
    const pair: ToolPair = {
      id: "e1",
      name: "bash",
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

  it("coalesces consecutive assistant events into one block when intervening tools are hidden by filter", () => {
    const prompt = userEv("p");
    const use1Block = toolUseBlock(1, "r1", "read", { path: "/a" }, "Read");
    const use1 = { type: "tool_use", id: "r1", name: "read", input: { path: "/a" } };
    const a1 = assistantEv(
      [textBlock(0, "first paragraph"), use1Block],
      200,
    );
    const a2 = assistantEv([textBlock(0, "second paragraph")], 400);
    const a3 = assistantEv([textBlock(0, "third paragraph")], 500);
    const pair: ToolPair = {
      id: "r1",
      name: "read",
      input: { path: "/a" },
      use: use1 as never,
      useEvent: a1,
      result: { type: "tool_result", tool_use_id: "r1", content: "body" },
      resultEvent: a1,
      isError: false,
      isPending: false,
    };
    const turn = mkTurn(prompt, [a1, a2, a3], { toolPairs: [pair] });
    // Read is hidden — the 3 assistant events should merge into one block.
    const filters = {
      hiddenSpeakers: new Set<"user" | "assistant" | "tool_result">(),
      hiddenTools: new Set(["read"]),
      errorsOnly: false,
      showThinking: true,
      showBookkeeping: true,
      showSidechain: true,
      filePath: "",
    };
    render(<TurnDetail turn={turn} showThinking={true} filters={filters} />);
    // All three paragraphs render, but there is only ONE "copy text"
    // button (one set of copy actions for the merged block).
    expect(screen.getByText("first paragraph")).toBeDefined();
    expect(screen.getByText("second paragraph")).toBeDefined();
    expect(screen.getByText("third paragraph")).toBeDefined();
    expect(screen.queryAllByRole("button", { name: /copy text/i })).toHaveLength(1);
    // No tool row when Read is hidden.
    expect(screen.queryByTestId("tool-pair-row")).toBeNull();
  });

  it("splits the assistant block when a visible tool separates two assistant events", () => {
    const prompt = userEv("p");
    const use1Block = toolUseBlock(1, "b1", "bash", { command: "ls" }, "Bash");
    const use1 = { type: "tool_use", id: "b1", name: "bash", input: { command: "ls" } };
    const a1 = assistantEv(
      [textBlock(0, "before tool"), use1Block],
      200,
    );
    const a2 = assistantEv([textBlock(0, "after tool")], 400);
    const pair: ToolPair = {
      id: "b1",
      name: "bash",
      input: { command: "ls" },
      use: use1 as never,
      useEvent: a1,
      result: { type: "tool_result", tool_use_id: "b1", content: "ok" },
      resultEvent: a1,
      isError: false,
      isPending: false,
    };
    const turn = mkTurn(prompt, [a1, a2], { toolPairs: [pair] });
    render(<TurnDetail turn={turn} showThinking={true} />);
    // Two separate copy-text buttons — one per assistant block.
    expect(screen.queryAllByRole("button", { name: /copy text/i })).toHaveLength(2);
  });

  it("never renders chips or count for empty (signature-only) thinking blocks", () => {
    const prompt = userEv("p");
    const turn = mkTurn(
      prompt,
      [
        assistantEv([
          thinkingBlock(0, ""),
          textBlock(1, "hi"),
        ]),
      ],
      // thinkingCount comes from grouping and we test it elsewhere; set
      // to 0 here since there's no useful thinking.
      { thinkingCount: 0 },
    );
    render(<TurnDetail turn={turn} showThinking={true} />);
    expect(screen.queryByText(/💭/)).toBeNull();
    expect(screen.queryByText(/redacted/i)).toBeNull();
  });

  it("Task tool exposes View agent log button that fires onOpenSubagent", async () => {
    const prompt = userEv("spawn");
    const useBlock = toolUseBlock(0, "t1", "task", { agent: "Explore", prompt: "find stuff" }, "Task");
    const use = {
      type: "tool_use",
      id: "t1",
      name: "task",
      input: { agent: "Explore", prompt: "find stuff" },
    };
    const asst = assistantEv([useBlock]);
    const pair: ToolPair = {
      id: "t1",
      name: "task",
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
