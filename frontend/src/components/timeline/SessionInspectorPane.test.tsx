import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { Turn } from "./grouping";
import type { TimelineEvent } from "../../api/types";
import { SessionInspectorPane } from "./SessionInspectorPane";

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

function turn(prompt: string): Turn {
  const p = userEv(prompt);
  return {
    id: p.byte_offset,
    userPrompt: p,
    events: [p],
    startTimestamp: p.timestamp,
    endTimestamp: p.timestamp,
    durationMs: 0,
    toolPairs: [],
    thinkingCount: 0,
    hasErrors: false,
  };
}

describe("SessionInspectorPane", () => {
  it("inline mode renders empty state when no turn selected", () => {
    render(
      <SessionInspectorPane
        turn={null}
        showThinking={true}
        asOverlay={false}
      />,
    );
    expect(screen.getByTestId("inspector-pane")).toBeDefined();
    expect(
      screen.getByText((t) =>
        t.toLowerCase().includes("select a turn from the timeline"),
      ),
    ).toBeDefined();
  });

  it("inline mode renders the selected turn's detail", () => {
    render(
      <SessionInspectorPane
        turn={turn("my prompt")}
        showThinking={true}
        asOverlay={false}
      />,
    );
    expect(screen.getByText(/my prompt/)).toBeDefined();
  });

  it("overlay mode renders a modal when a turn is selected", () => {
    render(
      <SessionInspectorPane
        turn={turn("phone prompt")}
        showThinking={true}
        asOverlay={true}
        onClose={() => {}}
      />,
    );
    expect(screen.getByTestId("inspector-overlay")).toBeDefined();
    expect(screen.getByText(/phone prompt/)).toBeDefined();
  });

  it("overlay mode renders nothing when no turn is selected", () => {
    render(
      <SessionInspectorPane
        turn={null}
        showThinking={true}
        asOverlay={true}
        onClose={() => {}}
      />,
    );
    expect(screen.queryByTestId("inspector-overlay")).toBeNull();
  });

  it("overlay Escape fires onClose", async () => {
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(
      <SessionInspectorPane
        turn={turn("close me")}
        showThinking={true}
        asOverlay={true}
        onClose={onClose}
      />,
    );
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });

  it("overlay backdrop click fires onClose; content click does not", async () => {
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(
      <SessionInspectorPane
        turn={turn("click test")}
        showThinking={true}
        asOverlay={true}
        onClose={onClose}
      />,
    );
    const backdrop = screen.getByTestId("inspector-overlay");
    await user.click(backdrop);
    expect(onClose).toHaveBeenCalled();

    onClose.mockClear();
    await user.click(screen.getByText(/click test/));
    expect(onClose).not.toHaveBeenCalled();
  });
});
