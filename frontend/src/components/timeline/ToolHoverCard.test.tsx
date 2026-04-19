import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { ToolPair } from "./grouping";
import { ToolHoverCard } from "./ToolHoverCard";
import { makeEvent } from "./test-helpers";

function ev() {
  return makeEvent("assistant", { byte_offset: 100 });
}

function mkPair(overrides: Partial<ToolPair> = {}): ToolPair {
  return {
    id: "t1",
    name: "read",
    input: { path: "/etc/hosts" },
    use: { type: "tool_use", id: "t1", name: "read" } as never,
    useEvent: ev(),
    result: { type: "tool_result", tool_use_id: "t1", content: "127.0.0.1 localhost" },
    resultEvent: ev(),
    isError: false,
    isPending: false,
    ...overrides,
  };
}

function makeAnchor(): HTMLElement {
  const el = document.createElement("div");
  document.body.appendChild(el);
  return el;
}

describe("ToolHoverCard", () => {
  it("renders the tool name and result", () => {
    render(
      <ToolHoverCard
        anchor={makeAnchor()}
        pair={mkPair()}
        pinned={false}
        onPin={() => {}}
        onClose={() => {}}
      />,
    );
    expect(screen.getByTestId("tool-hover-card")).toBeDefined();
    expect(screen.getAllByText("read")).toHaveLength(2);
    expect(screen.getByText(/127\.0\.0\.1 localhost/)).toBeDefined();
  });

  it("renders (pending) copy when no result yet", () => {
    render(
      <ToolHoverCard
        anchor={makeAnchor()}
        pair={mkPair({ result: null, resultEvent: null, isPending: true })}
        pinned={false}
        onPin={() => {}}
        onClose={() => {}}
      />,
    );
    expect(screen.getByText(/pending/i)).toBeDefined();
  });

  it("shows the 'click to pin' hint when not pinned", () => {
    render(
      <ToolHoverCard
        anchor={makeAnchor()}
        pair={mkPair()}
        pinned={false}
        onPin={() => {}}
        onClose={() => {}}
      />,
    );
    expect(screen.getByText(/click to pin/i)).toBeDefined();
  });

  it("shows the × close button only when pinned, and fires onClose", async () => {
    const onClose = vi.fn();
    const user = userEvent.setup();
    const { rerender } = render(
      <ToolHoverCard
        anchor={makeAnchor()}
        pair={mkPair()}
        pinned={false}
        onPin={() => {}}
        onClose={onClose}
      />,
    );
    expect(screen.queryByRole("button", { name: /close card/i })).toBeNull();

    rerender(
      <ToolHoverCard
        anchor={makeAnchor()}
        pair={mkPair()}
        pinned={true}
        onPin={() => {}}
        onClose={onClose}
      />,
    );
    await user.click(screen.getByRole("button", { name: /close card/i }));
    expect(onClose).toHaveBeenCalled();
  });

  it("clicking the card body while unpinned fires onPin", async () => {
    const onPin = vi.fn();
    const user = userEvent.setup();
    render(
      <ToolHoverCard
        anchor={makeAnchor()}
        pair={mkPair()}
        pinned={false}
        onPin={onPin}
        onClose={() => {}}
      />,
    );
    await user.click(screen.getByTestId("tool-hover-card"));
    expect(onPin).toHaveBeenCalled();
  });

  it("renders error state distinctively when pair.isError", () => {
    render(
      <ToolHoverCard
        anchor={makeAnchor()}
        pair={mkPair({
          result: {
            type: "tool_result",
            tool_use_id: "t1",
            content: "stderr",
            is_error: true,
          },
          isError: true,
        })}
        pinned={true}
        onPin={() => {}}
        onClose={() => {}}
      />,
    );
    const card = screen.getByTestId("tool-hover-card");
    expect(card.className).toContain("error");
    // error status chip in the header
    expect(screen.getAllByText(/error/i).length).toBeGreaterThan(0);
  });
});
