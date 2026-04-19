import { afterEach, describe, expect, it, vi } from "vitest";
import { render as rawRender, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactElement } from "react";

import * as apiClient from "../../api/client";
import { subscribeToAppCommands } from "../../state/AppCommands";
import { TurnRow } from "./TurnRow";
import { makePair, makeTurn } from "./test-helpers";
import { ContextMenuHost } from "../common/ContextMenu";

function render(ui: ReactElement) {
  return rawRender(
    <>
      {ui}
      <ContextMenuHost />
    </>,
  );
}

describe("TurnRow", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renders the backend-projected preview", () => {
    render(
      <TurnRow
        turn={makeTurn({ preview: "do the thing" })}
        selected={false}
        showThinking={true}
        onSelect={() => {}}
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
        onSelect={() => {}}
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
        onSelect={() => {}}
      />,
    );
    expect(screen.getByText(/💭3/)).toBeDefined();
    expect(screen.getByText("⚠")).toBeDefined();
  });

  it("falls back to a timestamped ref name when prompt text is missing", async () => {
    render(
      <TurnRow
        turn={makeTurn({ user_prompt_text: null })}
        selected={false}
        showThinking={true}
        onSelect={() => {}}
      />,
    );
    expect(screen.getByTestId("turn-row")).toBeDefined();
  });

  it("pins a turn and emits a refs refresh command", async () => {
    vi.spyOn(apiClient, "saveLibraryEntry").mockResolvedValue({
      slug: "turn",
      name: "turn",
      tags: ["turn"],
      created_at: null,
      body: "body",
      extras: {},
    });
    const seen: Array<unknown> = [];
    const unsubscribe = subscribeToAppCommands((command) => {
      seen.push(command);
    });

    render(
      <TurnRow
        turn={makeTurn({ user_prompt_text: "Investigate cache drift" })}
        selected={false}
        showThinking={true}
        onSelect={() => {}}
        repo="alpha"
      />,
    );
    const user = userEvent.setup();

    await user.pointer({
      keys: "[MouseRight]",
      target: screen.getByTestId("turn-row"),
    });
    await user.click(await screen.findByText("Pin turn as reference"));

    await waitFor(() =>
      expect(seen).toContainEqual({
        type: "library-changed",
        repo: "alpha",
        kind: "refs",
      }),
    );
    unsubscribe();
  });
});
