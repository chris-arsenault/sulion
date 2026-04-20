import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { TurnRow } from "./TurnRow";
import { makePair, makeTurn } from "./test-helpers";

const noop = () => {};

describe("TurnRow", () => {
  it("renders the backend-projected preview", () => {
    render(
      <TurnRow
        turn={makeTurn({ preview: "do the thing" })}
        selected={false}
        showThinking={true}
        onSelect={noop}
      />,
    );
    expect(screen.getByText(/do the thing/)).toBeDefined();
  });

  it("fires onSelect when clicked", async () => {
    const onSelect = vi.fn();
    const user = userEvent.setup();
    render(
      <TurnRow
        turn={makeTurn()}
        selected={false}
        showThinking={true}
        onSelect={onSelect}
      />,
    );
    await user.click(screen.getByRole("button"));
    expect(onSelect).toHaveBeenCalled();
  });

  it("shows tool badges with counts per tool type", () => {
    render(
      <TurnRow
        turn={makeTurn({
          tool_pairs: [
            makePair({ id: "a", name: "edit", category: "create_content" }),
            makePair({ id: "b", name: "edit", category: "create_content" }),
            makePair({ id: "c", name: "bash", category: "utility" }),
          ],
        })}
        selected={false}
        showThinking={true}
        onSelect={noop}
      />,
    );
    expect(screen.getByText("edit×2")).toBeDefined();
    expect(screen.getByText("bash")).toBeDefined();
  });

  it("shows thinking and error badges from projected metrics", () => {
    render(
      <TurnRow
        turn={makeTurn({ thinking_count: 3, has_errors: true })}
        selected={false}
        showThinking={true}
        onSelect={noop}
      />,
    );
    // Thinking badge renders the count alongside a sparkles sigil.
    expect(screen.getByText("3")).toBeDefined();
    // Error badge uses an alert-triangle sigil with an accessible label.
    expect(
      document.querySelector(".tr__badge--error"),
    ).not.toBeNull();
  });
});
