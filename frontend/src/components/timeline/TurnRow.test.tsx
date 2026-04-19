import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { Turn, ToolPair } from "./grouping";
import type { TimelineEvent } from "../../api/types";
import { TurnRow } from "./TurnRow";

function userEv(text: string): TimelineEvent {
  return {
    byte_offset: 100,
    timestamp: "2025-01-01T00:00:00Z",
    kind: "user",
    payload: {
      type: "user",
      message: { role: "user", content: [{ type: "text", text }] },
    },
  };
}

function assistantEv(text = "reply"): TimelineEvent {
  return {
    byte_offset: 200,
    timestamp: "2025-01-01T00:00:02Z",
    kind: "assistant",
    payload: {
      type: "assistant",
      message: { role: "assistant", content: [{ type: "text", text }] },
    },
  };
}

function turn(overrides: Partial<Turn> = {}): Turn {
  const prompt = userEv("do the thing");
  return {
    id: prompt.byte_offset,
    userPrompt: prompt,
    events: [prompt, assistantEv()],
    startTimestamp: prompt.timestamp,
    endTimestamp: "2025-01-01T00:00:02Z",
    durationMs: 2000,
    toolPairs: [],
    thinkingCount: 0,
    hasErrors: false,
    ...overrides,
  };
}

describe("TurnRow", () => {
  it("renders the user prompt preview", () => {
    render(
      <TurnRow
        turn={turn()}
        selected={false}
        showThinking={true}
        onSelect={() => {}}
      />,
    );
    expect(screen.getByText(/do the thing/)).toBeDefined();
  });

  it("fires onSelect when clicked", async () => {
    const onSelect = vi.fn();
    const user = userEvent.setup();
    render(
      <TurnRow
        turn={turn()}
        selected={false}
        showThinking={true}
        onSelect={onSelect}
      />,
    );
    await user.click(screen.getByRole("button"));
    expect(onSelect).toHaveBeenCalled();
  });

  it("reflects selected state via aria-pressed and a class", () => {
    render(
      <TurnRow
        turn={turn()}
        selected={true}
        showThinking={true}
        onSelect={() => {}}
      />,
    );
    const btn = screen.getByRole("button");
    expect(btn.getAttribute("aria-pressed")).toBe("true");
    expect(btn.className).toContain("selected");
  });

  it("shows tool badges with counts per tool type", () => {
    const editPair: ToolPair = {
      id: "a",
      name: "Edit",
      input: {},
      use: { type: "tool_use", id: "a", name: "Edit" } as never,
      useEvent: assistantEv(),
      result: null,
      resultEvent: null,
      isError: false,
      isPending: false,
    };
    const editPair2: ToolPair = { ...editPair, id: "b" };
    const bashPair: ToolPair = {
      id: "c",
      name: "Bash",
      input: {},
      use: { type: "tool_use", id: "c", name: "Bash" } as never,
      useEvent: assistantEv(),
      result: null,
      resultEvent: null,
      isError: false,
      isPending: false,
    };
    render(
      <TurnRow
        turn={turn({ toolPairs: [editPair, editPair2, bashPair] })}
        selected={false}
        showThinking={true}
        onSelect={() => {}}
      />,
    );
    expect(screen.getByText("Edit ×2")).toBeDefined();
    expect(screen.getByText("Bash")).toBeDefined();
  });

  it("hides the thinking badge when showThinking=false", () => {
    render(
      <TurnRow
        turn={turn({ thinkingCount: 3 })}
        selected={false}
        showThinking={false}
        onSelect={() => {}}
      />,
    );
    expect(screen.queryByText(/💭/)).toBeNull();
  });

  it("shows error indicator when the turn has errors", () => {
    render(
      <TurnRow
        turn={turn({ hasErrors: true })}
        selected={false}
        showThinking={true}
        onSelect={() => {}}
      />,
    );
    expect(screen.getByText("⚠")).toBeDefined();
  });

  it("falls back to an orphan preview when there's no user prompt", () => {
    const asst = assistantEv("boot sequence");
    const t: Turn = {
      id: asst.byte_offset,
      userPrompt: null,
      events: [asst],
      startTimestamp: asst.timestamp,
      endTimestamp: asst.timestamp,
      durationMs: 0,
      toolPairs: [],
      thinkingCount: 0,
      hasErrors: false,
    };
    render(
      <TurnRow
        turn={t}
        selected={false}
        showThinking={true}
        onSelect={() => {}}
      />,
    );
    expect(screen.getByText(/boot sequence/)).toBeDefined();
  });
});
