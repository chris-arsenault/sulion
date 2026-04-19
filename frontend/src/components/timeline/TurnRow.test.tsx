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
    expect(screen.getByText("Edit×2")).toBeDefined();
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

  it("multi-paragraph prompt: shows first paragraph only, with trailing ellipsis when more exists", () => {
    const multi = {
      byte_offset: 100,
      timestamp: "2025-01-01T00:00:00Z",
      kind: "user",
      payload: {
        type: "user",
        message: {
          role: "user",
          content: [
            {
              type: "text",
              text: "Fix the bug in foo.ts.\n\nContext: the user clicks delete and nothing happens. The handler fires but the DELETE never reaches the server.\n\nCheck network tab for what actually went over the wire.",
            },
          ],
        },
      },
    };
    render(
      <TurnRow
        turn={turn({ userPrompt: multi as never, events: [multi as never] })}
        selected={false}
        showThinking={true}
        onSelect={() => {}}
      />,
    );
    // First paragraph only
    expect(screen.getByText((t) => t.includes("Fix the bug in foo.ts."))).toBeDefined();
    // Second+ paragraph content NOT in the row
    expect(screen.queryByText(/Context: the user clicks/)).toBeNull();
    expect(screen.queryByText(/Check network tab/)).toBeNull();
    // Trailing ellipsis hint
    expect(
      screen.getByText((t) => t.endsWith(" …") || t.endsWith("…")),
    ).toBeDefined();
  });

  it("single-paragraph prompt with internal line breaks: collapses to one paragraph, no trailing ellipsis", () => {
    const singleWithLinebreaks = {
      byte_offset: 100,
      timestamp: "2025-01-01T00:00:00Z",
      kind: "user",
      payload: {
        type: "user",
        message: {
          role: "user",
          content: [
            {
              type: "text",
              // One paragraph wrapped with single newlines (not blank line)
              text: "add a new session rename feature\nthat lets me name sessions\nwith emoji",
            },
          ],
        },
      },
    };
    render(
      <TurnRow
        turn={turn({
          userPrompt: singleWithLinebreaks as never,
          events: [singleWithLinebreaks as never],
        })}
        selected={false}
        showThinking={true}
        onSelect={() => {}}
      />,
    );
    // Whole thing visible, whitespace collapsed, NO trailing " …" since
    // there's only one paragraph.
    expect(
      screen.getByText(
        "add a new session rename feature that lets me name sessions with emoji",
      ),
    ).toBeDefined();
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
