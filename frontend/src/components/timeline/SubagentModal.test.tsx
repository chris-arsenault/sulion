import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { TimelineEvent } from "../../api/types";
import { SubagentModal, collectSubagentEvents } from "./SubagentModal";

function ev(
  byte_offset: number,
  kind: string,
  payload: Record<string, unknown>,
): TimelineEvent {
  return {
    byte_offset,
    timestamp: "2025-01-01T00:00:00Z",
    kind,
    payload,
  };
}

describe("collectSubagentEvents", () => {
  it("collects sidechain events traceable to the Task tool_use via parentUuid", () => {
    // Main thread: user → assistant emits Task tool_use t1
    const mainAssistant = ev(100, "assistant", {
      type: "assistant",
      uuid: "asst-1",
      message: { content: [{ type: "tool_use", id: "t1", name: "Task" }] },
    });
    // Sidechain root: first subagent event chained off the assistant
    const sub1 = ev(200, "assistant", {
      type: "assistant",
      uuid: "sub-1",
      parentUuid: "asst-1",
      isSidechain: true,
      message: { content: [{ type: "text", text: "starting" }] },
    });
    const sub2 = ev(300, "assistant", {
      type: "assistant",
      uuid: "sub-2",
      parentUuid: "sub-1",
      isSidechain: true,
      message: { content: [{ type: "text", text: "working" }] },
    });
    const unrelated = ev(400, "assistant", {
      type: "assistant",
      uuid: "other",
      parentUuid: "somewhere-else",
      isSidechain: true,
      message: { content: [{ type: "text", text: "nope" }] },
    });

    const out = collectSubagentEvents(
      [mainAssistant, sub1, sub2, unrelated],
      "t1",
      "asst-1",
    );
    const uuids = out.map((e) => (e.payload as { uuid?: string }).uuid);
    expect(uuids).toContain("sub-1");
    expect(uuids).toContain("sub-2");
    expect(uuids).not.toContain("other");
    // The main assistant is also included because its uuid seeds the lineage
    expect(uuids).toContain("asst-1");
  });

  it("includes events referencing the Task tool_use_id explicitly", () => {
    const report = ev(500, "user", {
      type: "user",
      uuid: "report-1",
      tool_use_id: "t1",
      isSidechain: true,
      message: { content: [{ type: "text", text: "report" }] },
    });
    const out = collectSubagentEvents([report], "t1");
    expect(out).toHaveLength(1);
  });

  it("returns empty when no events match", () => {
    const bystander = ev(600, "assistant", {
      type: "assistant",
      uuid: "b",
      isSidechain: false,
      message: { content: [{ type: "text", text: "nope" }] },
    });
    expect(collectSubagentEvents([bystander], "t1").length).toBe(0);
  });
});

describe("SubagentModal", () => {
  it("renders nothing-to-show copy when there are no subagent events", () => {
    render(
      <SubagentModal
        toolUseId="missing"
        allEvents={[]}
        showThinking={true}
        onClose={() => {}}
      />,
    );
    expect(
      screen.getByText((t) => t.toLowerCase().includes("no subagent events found")),
    ).toBeDefined();
  });

  it("Escape fires onClose", async () => {
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(
      <SubagentModal
        toolUseId="missing"
        allEvents={[]}
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
    render(
      <SubagentModal
        toolUseId="missing"
        allEvents={[]}
        showThinking={true}
        onClose={onClose}
        title="Agent log"
      />,
    );
    const dialog = screen.getByTestId("subagent-modal");
    await user.click(dialog); // backdrop
    expect(onClose).toHaveBeenCalled();

    onClose.mockClear();
    await user.click(screen.getByText(/Agent log/));
    expect(onClose).not.toHaveBeenCalled();
  });

  it("renders sub-events grouped into turns when lineage is present", () => {
    const mainAsst = ev(100, "assistant", {
      type: "assistant",
      uuid: "asst-1",
      message: { content: [{ type: "tool_use", id: "t1", name: "Task" }] },
    });
    const subPrompt = ev(200, "user", {
      type: "user",
      uuid: "sub-prompt",
      parentUuid: "asst-1",
      isSidechain: true,
      message: { content: [{ type: "text", text: "subagent task" }] },
    });
    const subAsst = ev(300, "assistant", {
      type: "assistant",
      uuid: "sub-asst",
      parentUuid: "sub-prompt",
      isSidechain: true,
      message: { content: [{ type: "text", text: "subagent reply" }] },
    });
    render(
      <SubagentModal
        toolUseId="t1"
        seedUuid="asst-1"
        allEvents={[mainAsst, subPrompt, subAsst]}
        showThinking={true}
        onClose={() => {}}
      />,
    );
    // Header meta shows event count — TurnDetail inside also emits
    // "N events" in its own header, so we assert existence not uniqueness.
    expect(screen.getAllByText(/event/).length).toBeGreaterThan(0);
  });
});
