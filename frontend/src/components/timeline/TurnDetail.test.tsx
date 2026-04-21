import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";

import * as apiClient from "../../api/client";
import { subscribeToAppCommands } from "../../state/AppCommands";
import { TurnDetail } from "./TurnDetail";
import { ContextMenuHost } from "../common/ContextMenu";
import {
  assistantChunk,
  assistantItems,
  makePair,
  makeSubagent,
  makeTurn,
  toolChunk,
} from "./test-helpers";

describe("TurnDetail", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  function renderWithContextMenu(ui: ReactNode) {
    return render(
      <>
        {ui}
        <ContextMenuHost />
      </>,
    );
  }

  it("renders prompt, assistant text, and projected tool rows", () => {
    const pair = makePair({
      id: "t1",
      name: "bash",
      input: { command: "pwd" },
      result: { content: "/tmp", is_error: false },
    });
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          user_prompt_text: "my prompt",
          tool_pairs: [pair],
          operation_count: 1,
          chunks: [
            assistantChunk(assistantItems("before", { tool: "t1" }, "after")),
            toolChunk("t1"),
          ],
        })}
        showThinking={true}
      />,
    );
    expect(screen.getByText(/my prompt/)).toBeDefined();
    expect(screen.getByText(/before/)).toBeDefined();
    expect(screen.getByText("bash")).toBeDefined();
  });

  it("hides thinking chips when showThinking=false", () => {
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          thinking_count: 1,
          chunks: [assistantChunk(assistantItems("reply"), ["private thought"])],
        })}
        showThinking={false}
      />,
    );
    expect(screen.queryByText(/thinking/i)).toBeNull();
  });

  it("shows tool error state and result body", () => {
    const pair = makePair({
      id: "e1",
      name: "edit",
      category: "create_content",
      input: { path: "/tmp/file.txt" },
      result: { content: "permission denied", is_error: true },
      is_error: true,
    });
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          tool_pairs: [pair],
          has_errors: true,
          operation_count: 1,
          chunks: [toolChunk("e1")],
        })}
        showThinking={true}
      />,
    );
    expect(screen.getByText(/errors/i)).toBeDefined();
    expect(screen.getByText(/permission denied/i)).toBeDefined();
  });

  it("renders canonical edit payloads without an empty result placeholder", async () => {
    const pair = makePair({
      id: "edit-1",
      name: "edit",
      category: "create_content",
      input: { path: "/tmp/file.txt" },
      result: {
        content: null,
        payload: {
          path: "/tmp/file.txt",
          old_text: "before",
          new_text: "after",
        },
        is_error: false,
      },
    });
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          tool_pairs: [pair],
          operation_count: 1,
          chunks: [toolChunk("edit-1")],
        })}
        showThinking={true}
      />,
    );
    const user = userEvent.setup();
    await user.click(screen.getByLabelText(/expand tool details/i));
    expect(screen.getByText("before")).toBeDefined();
    expect(screen.getByText("after")).toBeDefined();
    expect(screen.queryByText(/\(empty result\)/i)).toBeNull();
  });

  it("opens subagent log button only for task pairs with projected subagent data", async () => {
    const onOpen = vi.fn();
    const user = userEvent.setup();
    const pair = makePair({
      id: "task-1",
      name: "task",
      category: "delegate",
      input: { description: "delegate work" },
      subagent: makeSubagent({ title: "Agent log · delegate work" }),
    });
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          tool_pairs: [pair],
          operation_count: 1,
          chunks: [toolChunk("task-1")],
        })}
        showThinking={true}
        onOpenSubagent={onOpen}
      />,
    );
    await user.click(screen.getByText(/view agent log/i));
    expect(onOpen).toHaveBeenCalledWith(pair);
  });

  it("saves the prompt as a reusable prompt", async () => {
    vi.spyOn(apiClient, "saveLibraryEntry").mockResolvedValue({
      slug: "prompt",
      name: "Prompt: my prompt",
      created_at: null,
      updated_at: null,
      body: "my prompt",
    });
    const seen: Array<unknown> = [];
    const unsubscribe = subscribeToAppCommands((command) => {
      seen.push(command);
    });

    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({ user_prompt_text: "my prompt" })}
        showThinking={true}
      />,
    );
    const user = userEvent.setup();

    await user.pointer({
      keys: "[MouseRight]",
      target: screen.getByText(/my prompt/),
    });
    await user.click(screen.getByRole("menuitem", { name: /save as prompt/i }));

    expect(apiClient.saveLibraryEntry).toHaveBeenCalledWith("prompts", {
      name: "Prompt: my prompt",
      body: "my prompt",
    });
    expect(seen).toContainEqual({ type: "library-changed", kind: "prompts" });
    unsubscribe();
  });

  it("saves assistant output as a reference", async () => {
    vi.spyOn(apiClient, "saveLibraryEntry").mockResolvedValue({
      slug: "reference",
      name: "Reference: before",
      created_at: null,
      updated_at: null,
      body: "before",
    });
    const seen: Array<unknown> = [];
    const unsubscribe = subscribeToAppCommands((command) => {
      seen.push(command);
    });

    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          chunks: [assistantChunk(assistantItems("before"))],
        })}
        showThinking={true}
      />,
    );
    const user = userEvent.setup();

    await user.pointer({
      keys: "[MouseRight]",
      target: screen.getByText("before"),
    });
    expect(screen.getByRole("menuitem", { name: /copy text/i })).toBeDefined();
    expect(screen.getByRole("menuitem", { name: /copy event/i })).toBeDefined();
    await user.click(screen.getByRole("menuitem", { name: /save as reference/i }));

    expect(apiClient.saveLibraryEntry).toHaveBeenCalledWith("references", {
      name: "Reference: before",
      body: "before",
    });
    expect(seen).toContainEqual({ type: "library-changed", kind: "references" });
    unsubscribe();
  });

  it("marks the focused tool, expands it, and collapses its siblings", () => {
    const focused = makePair({
      id: "tool_focus",
      name: "edit",
      input: { path: "src/lib.rs" },
      result: { content: "ok", is_error: false },
    });
    const sibling = makePair({
      id: "tool_other",
      name: "read",
      input: { path: "README.md" },
      result: { content: "…", is_error: false },
    });
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          tool_pairs: [focused, sibling],
          operation_count: 2,
          chunks: [toolChunk("tool_focus"), toolChunk("tool_other")],
        })}
        showThinking={true}
        focusPairId="tool_focus"
        focusKey="focus-1"
      />,
    );

    const rows = screen.getAllByTestId("tool-pair-row");
    const focusedRow = rows.find(
      (el) => el.getAttribute("data-pair-id") === "tool_focus",
    );
    const siblingRow = rows.find(
      (el) => el.getAttribute("data-pair-id") === "tool_other",
    );

    expect(focusedRow?.className).toContain("td__tool--focused");
    expect(focusedRow?.getAttribute("data-focused")).toBe("true");
    expect(siblingRow?.className ?? "").not.toContain("td__tool--focused");

    // Expanded rows render a `.td__tool-body`; collapsed rows do not.
    expect(focusedRow?.querySelector(".td__tool-body")).not.toBeNull();
    expect(siblingRow?.querySelector(".td__tool-body")).toBeNull();
  });
});
