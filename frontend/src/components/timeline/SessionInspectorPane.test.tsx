import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { SessionInspectorPane } from "./SessionInspectorPane";
import { assistantChunk, makeTurn } from "./test-helpers";

const noop = () => {};

describe("SessionInspectorPane", () => {
  it("inline mode renders empty state when no turn is selected", () => {
    render(
      <SessionInspectorPane
        turn={null}
        showThinking={true}
        asOverlay={false}
      />,
    );
    expect(screen.getByTestId("inspector-pane")).toBeDefined();
    expect(
      screen.getByText((text) =>
        text.toLowerCase().includes("select a turn from the timeline"),
      ),
    ).toBeDefined();
  });

  it("inline mode renders the selected turn detail", () => {
    render(
      <SessionInspectorPane
        turn={makeTurn({
          user_prompt_text: "my prompt",
          chunks: [assistantChunk([{ kind: "text", text: "reply" }])],
        })}
        showThinking={true}
        asOverlay={false}
      />,
    );
    expect(screen.getByText(/my prompt/)).toBeDefined();
    expect(screen.getByText(/reply/)).toBeDefined();
  });

  it("overlay mode renders a modal when a turn is selected", () => {
    render(
      <SessionInspectorPane
        turn={makeTurn({ user_prompt_text: "phone prompt" })}
        showThinking={true}
        asOverlay={true}
        onClose={noop}
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
        onClose={noop}
      />,
    );
    expect(screen.queryByTestId("inspector-overlay")).toBeNull();
  });

  it("overlay Escape fires onClose", async () => {
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(
      <SessionInspectorPane
        turn={makeTurn({ user_prompt_text: "close me" })}
        showThinking={true}
        asOverlay={true}
        onClose={onClose}
      />,
    );
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });
});
