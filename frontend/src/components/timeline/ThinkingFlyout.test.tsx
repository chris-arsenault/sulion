import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { ThinkingFlyout } from "./ThinkingFlyout";

function makeAnchor(): HTMLElement {
  const el = document.createElement("div");
  el.textContent = "anchor";
  document.body.appendChild(el);
  return el;
}

describe("ThinkingFlyout", () => {
  it("renders the thinking text in a portalled card", () => {
    render(
      <ThinkingFlyout
        anchor={makeAnchor()}
        thinkingText="my inner monologue"
        onClose={() => {}}
      />,
    );
    expect(screen.getByTestId("thinking-flyout")).toBeDefined();
    expect(screen.getByText("my inner monologue")).toBeDefined();
  });

  it("fires onClose on Escape", async () => {
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(
      <ThinkingFlyout
        anchor={makeAnchor()}
        thinkingText="x"
        onClose={onClose}
      />,
    );
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });

  it("fires onClose on × click", async () => {
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(
      <ThinkingFlyout
        anchor={makeAnchor()}
        thinkingText="x"
        onClose={onClose}
      />,
    );
    await user.click(screen.getByRole("button", { name: /close thinking/i }));
    expect(onClose).toHaveBeenCalled();
  });
});
