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
  it("styles plan commands and preserves a failed result", () => {
    render(<ToolHoverCard anchor={document.body} pinned={false} onPin={noop} onClose={noop}
      pair={pair({ operation_type: "sulion_plan", category: "plan", is_error: true,
        input: { command: "sulion plan close --completed", plan_commands: [{ action: "close", status: "completed" }] },
        result: { content: "Plan still has pending phases", is_error: true },
      })} />);
    expect(screen.getByTestId("tool-hover-card").className).toContain("thc--plan");
    expect(screen.getByText("Requested status")).toBeDefined();
    expect(screen.getByText("Plan still has pending phases")).toBeDefined();
    expect(screen.getByText("error")).toBeDefined();
  });
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

  it("renders canonical edit payloads without an empty result block", () => {
    render(
      <ToolHoverCard
        anchor={document.body}
        pair={pair({
          name: "edit",
          input: {
            file_edits: [
              {
                path: "/tmp/file.txt",
                operation: "update",
                in_out: { old_text: "before", new_text: "after" },
              },
            ],
          },
          result: {
            content: null,
            payload: null,
            is_error: false,
          },
        })}
        pinned={false}
        onPin={noop}
        onClose={noop}
      />,
    );
    expect(screen.getByText("before")).toBeDefined();
    expect(screen.getByText("after")).toBeDefined();
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
