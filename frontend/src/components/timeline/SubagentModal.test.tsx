import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { Turn } from "./grouping";
import type { TimelineTurnSummary } from "../../api/types";
import { useTurnStream } from "./useTurnStream";

vi.mock("./useTurnStream", () => ({ useTurnStream: vi.fn() }));
import { SubagentModal } from "./SubagentModal";
import { assistantChunk, itemsOf, makeSubagent, makeTurn, toolChunk } from "./test-helpers";

const noop = () => {};
const NO_TURNS: TimelineTurnSummary[] = [];
const NESTED_TASK_TURNS: Turn[] = [
  makeTurn({
    tool_pairs: [
      {
        id: "task-2",
        name: "Task",
        operation_type: "task",
        is_error: false,
        is_pending: false,
        file_touches: [],
        subagent: makeSubagent({ title: "inner agent" }),
      },
    ],
    items: itemsOf(toolChunk("task-2")),
  }),
];
const REPLY_TURNS: Turn[] = [
  makeTurn({
    user_prompt_text: "subagent task",
    preview: "subagent task",
    items: itemsOf(assistantChunk([{ kind: "text", text: "subagent reply" }])),
  }),
];

describe("SubagentModal", () => {
  it("renders empty copy when there are no subagent turns", () => {
    render(
      <SubagentModal
        subagent={makeSubagent({ event_count: 0 })}
        turns={NO_TURNS}
        showThinking={true}
        onClose={noop}
      />,
    );
    expect(
      screen.getByText((text) => text.toLowerCase().includes("no subagent events found")),
    ).toBeDefined();
  });

  it("shows the reference's counts while its turns load", () => {
    render(
      <SubagentModal
        subagent={makeSubagent({ event_count: 7, turn_count: 2 })}
        turns={null}
        showThinking={true}
        onClose={noop}
      />,
    );
    expect(screen.getByText(/loading subagent turns/i)).toBeDefined();
    expect(screen.getByText(/7 events · 2 turns/)).toBeDefined();
  });

  it("Escape fires onClose", async () => {
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(
      <SubagentModal
        subagent={makeSubagent()}
        turns={NO_TURNS}
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
        turns={NO_TURNS}
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
        turns={NO_TURNS}
        showThinking={true}
        onClose={noop}
      />,
    );
    expect(screen.queryByRole("button", { name: /parent agent/i })).toBeNull();
    rerender(
      <SubagentModal
        subagent={makeSubagent()}
        turns={NO_TURNS}
        showThinking={true}
        onClose={noop}
        onBack={onBack}
      />,
    );
    await user.click(screen.getByRole("button", { name: /parent agent/i }));
    expect(onBack).toHaveBeenCalled();
  });

  it("nested task pairs expose their own agent log link", () => {
    vi.mocked(useTurnStream).mockReturnValue({ turn: NESTED_TASK_TURNS[0], revision: 0, loading: false });
    render(
      <SubagentModal
        subagent={makeSubagent()}
        turns={NESTED_TASK_TURNS.map((turn) => ({ ...turn, operation_badges: [] }))}
        showThinking={true}
        onClose={noop}
        onOpenSubagent={noop}
      />,
    );
    expect(screen.getByText(/view agent log/i)).toBeDefined();
  });

  it("renders the referenced turns", () => {
    vi.mocked(useTurnStream).mockReturnValue({ turn: REPLY_TURNS[0], revision: 0, loading: false });
    render(
      <SubagentModal
        subagent={makeSubagent({ event_count: 2 })}
        turns={REPLY_TURNS.map((turn) => ({ ...turn, operation_badges: [] }))}
        showThinking={true}
        onClose={noop}
      />,
    );
    expect(screen.getByText(/subagent task/)).toBeDefined();
    expect(screen.getByText(/subagent reply/)).toBeDefined();
  });
});
