import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { ToolPair } from "./grouping";
import { ToolHoverCard } from "./ToolHoverCard";
import { makePair } from "./test-helpers";

const noop = () => {};

function pair(overrides: Partial<ToolPair> = {}): ToolPair {
  return makePair({
    id: "t1",
    name: "bash",
    input: { command: "cat /etc/hosts" },
    result: { content: "127.0.0.1 localhost", is_error: false },
    ...overrides,
  });
}

describe("ToolHoverCard", () => {
  it("renders tool input and result", () => {
    render(
      <ToolHoverCard
        anchor={document.body}
        pair={pair()}
        pinned={false}
        onPin={noop}
        onClose={noop}
      />,
    );
    expect(screen.getByText("bash")).toBeDefined();
    expect(screen.getByText(/127.0.0.1 localhost/)).toBeDefined();
  });

  it("shows pending state distinctly", () => {
    render(
      <ToolHoverCard
        anchor={document.body}
        pair={pair({ result: null, is_pending: true })}
        pinned={false}
        onPin={noop}
        onClose={noop}
      />,
    );
    expect(screen.getByText(/pending/i)).toBeDefined();
  });

  it("click-to-pin calls onPin and pinned close button calls onClose", async () => {
    const onPin = vi.fn();
    const onClose = vi.fn();
    const user = userEvent.setup();

    const { rerender } = render(
      <ToolHoverCard
        anchor={document.body}
        pair={pair()}
        pinned={false}
        onPin={onPin}
        onClose={onClose}
      />,
    );
    await user.click(screen.getByLabelText(/pin card open/i));
    expect(onPin).toHaveBeenCalled();

    rerender(
      <ToolHoverCard
        anchor={document.body}
        pair={pair()}
        pinned={true}
        onPin={onPin}
        onClose={onClose}
      />,
    );
    await user.click(screen.getByLabelText(/close card/i));
    expect(onClose).toHaveBeenCalled();
  });
});
