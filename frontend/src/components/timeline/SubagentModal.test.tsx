import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { SubagentModal } from "./SubagentModal";
import { assistantChunk, makeSubagent, makeTurn } from "./test-helpers";

const noop = () => {};

describe("SubagentModal", () => {
  it("renders empty copy when there are no subagent turns", () => {
    render(
      <SubagentModal
        subagent={makeSubagent({ event_count: 0, turns: [] })}
        showThinking={true}
        onClose={noop}
      />,
    );
    expect(
      screen.getByText((text) => text.toLowerCase().includes("no subagent events found")),
    ).toBeDefined();
  });

  it("Escape fires onClose", async () => {
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(
      <SubagentModal
        subagent={makeSubagent()}
        showThinking={true}
        onClose={onClose}
      />,
    );
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });

  it("clicking backdrop fires onClose; content click does not", async () => {
    const onClose = vi.fn();
    const user = userEvent.setup();
    const { container } = render(
      <SubagentModal
        subagent={makeSubagent({ title: "Agent log" })}
        showThinking={true}
        onClose={onClose}
      />,
    );
    const scrim = container.ownerDocument.querySelector(
      ".ui-overlay__scrim",
    ) as HTMLElement | null;
    expect(scrim).not.toBeNull();
    await user.click(scrim!);
    expect(onClose).toHaveBeenCalled();

    onClose.mockClear();
    await user.click(screen.getByText(/Agent log/));
    expect(onClose).not.toHaveBeenCalled();
  });

  it("shows a back button only when nested, and it fires onBack", async () => {
    const onBack = vi.fn();
    const user = userEvent.setup();
    const { rerender } = render(
      <SubagentModal
        subagent={makeSubagent()}
        showThinking={true}
        onClose={noop}
      />,
    );
    expect(screen.queryByRole("button", { name: /parent agent/i })).toBeNull();
    rerender(
      <SubagentModal
        subagent={makeSubagent()}
        showThinking={true}
        onClose={noop}
        onBack={onBack}
      />,
    );
    await user.click(screen.getByRole("button", { name: /parent agent/i }));
    expect(onBack).toHaveBeenCalled();
  });

  it("nested task pairs expose their own agent log link", () => {
    const nested = makeSubagent({ title: "inner agent" });
    render(
      <SubagentModal
        subagent={makeSubagent({
          turns: [
            makeTurn({
              tool_pairs: [
                {
                  id: "task-2",
                  name: "Task",
                  operation_type: "task",
                  is_error: false,
                  is_pending: false,
                  file_touches: [],
                  subagent: nested,
                },
              ],
              chunks: [{ kind: "tool", pair_id: "task-2" }],
            }),
          ],
        })}
        showThinking={true}
        onClose={noop}
        onOpenSubagent={noop}
      />,
    );
    expect(screen.getByText(/view agent log/i)).toBeDefined();
  });

  it("renders projected turns", () => {
    render(
      <SubagentModal
        subagent={makeSubagent({
          event_count: 2,
          turns: [
            makeTurn({
              user_prompt_text: "subagent task",
              preview: "subagent task",
              chunks: [assistantChunk([{ kind: "text", text: "subagent reply" }])],
            }),
          ],
        })}
        showThinking={true}
        onClose={noop}
      />,
    );
    expect(screen.getByText(/subagent task/)).toBeDefined();
    expect(screen.getByText(/subagent reply/)).toBeDefined();
  });
});
