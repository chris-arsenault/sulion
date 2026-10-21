import { afterEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";

import * as apiClient from "../../api/client";
import { subscribeToAppCommands } from "../../state/AppCommands";
import { TurnDetail } from "./TurnDetail";
import { ContextMenuHost } from "../common/ContextMenu";
import { Overlay } from "../ui";
import {
  assistantChunk,
  assistantItems,
  makePair,
  makeSubagent,
  itemsOf,
  makeTurn,
  toolChunk,
} from "./test-helpers";

describe("TurnDetail", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  function stubMobileViewport() {
    vi.stubGlobal(
      "matchMedia",
      vi.fn().mockImplementation((query: string) => ({
        matches: query === "(max-width: 767px)",
        media: query,
        onchange: null,
        addListener: vi.fn(),
        removeListener: vi.fn(),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    );
  }

  function renderWithContextMenu(ui: ReactNode) {
    return render(
      <>
        {ui}
        <ContextMenuHost />
      </>,
    );
  }

  it("shows agent replies as collapsed routed messages with Markdown on demand", async () => {
    render(<TurnDetail turn={makeTurn({ items: [{ offset: 1, kind: "agent_message",
      author: "/root/reviewer", recipient: "/root", message_type: "FINAL_ANSWER",
      text: "**Findings**\n\n| Area | Result |\n| --- | --- |\n| Parser | OK |",
    }] })} showThinking />);
    const summary = screen.getByText("Agent reply").closest("summary")!;
    expect(summary.textContent).toContain("/root/reviewer → /root");
    expect(screen.queryByText("Findings")).toBeNull();
    const user = userEvent.setup();
    await user.click(summary);
    expect((await screen.findByText("Findings")).tagName).toBe("STRONG");
    expect(screen.getByRole("table")).toBeDefined();
    await user.click(summary);
    expect(screen.queryByText("Findings")).toBeNull();
  });

  it("labels encrypted-only agent messages without inventing their content", async () => {
    render(<TurnDetail turn={makeTurn({ items: [{ offset: 1, kind: "agent_message",
      author: "/root/worker", recipient: "/root", message_type: null,
      text: "[Encrypted agent message; plaintext unavailable]",
    }] })} showThinking />);
    await userEvent.setup().click(screen.getByText("Agent message").closest("summary")!);
    expect(await screen.findByText("[Encrypted agent message; plaintext unavailable]")).toBeDefined();
  });

  it("limits long prompts to 240 characters and expands the full Markdown on demand", async () => {
    const prompt = `${"x".repeat(239)}😀 **last sentence**`;
    const { rerender } = render(<TurnDetail turn={makeTurn({ user_prompt_text: prompt })} showThinking />);
    const preview = document.querySelector(".td__prompt-preview");
    expect(preview?.textContent).toBe(`${"x".repeat(239)}😀…`);
    expect(screen.queryByText("last sentence")).toBeNull();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Show full prompt" }));
    expect(screen.getByText("last sentence").tagName).toBe("STRONG");
    expect(screen.getByRole("button", { name: "Show less" }).getAttribute("aria-expanded")).toBe("true");
    rerender(<TurnDetail turn={makeTurn({ user_prompt_text: prompt, event_count: 5 })} showThinking />);
    expect(screen.getByText("last sentence")).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Show less" }));
    expect(screen.queryByText("last sentence")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Show full prompt" }));
    rerender(<TurnDetail turn={makeTurn({ id: 2, user_prompt_text: prompt })} showThinking />);
    expect(screen.queryByText("last sentence")).toBeNull();
  });

  it("leaves short and filtered prompts without an expansion control", () => {
    const { rerender } = render(<TurnDetail turn={makeTurn({ user_prompt_text: "x".repeat(240) })} showThinking />);
    expect(screen.queryByRole("button", { name: "Show full prompt" })).toBeNull();
    rerender(<TurnDetail turn={makeTurn({ user_prompt_text: "x".repeat(1000) })} showThinking hideUserPrompt />);
    expect(screen.queryByRole("button", { name: "Show full prompt" })).toBeNull();
  });

  it("opens mobile assistant output in the shared modal and restores scroll and tool expansion", async () => {
    stubMobileViewport();
    const pair = makePair({ id: "tool", name: "read", input: { path: "one.txt" } });
    const turn = makeTurn({ tool_pairs: [pair], items: itemsOf(assistantChunk(assistantItems("Read this output")), toolChunk("tool")) });
    const { rerender } = render(<TurnDetail turn={turn} showThinking />);
    const user = userEvent.setup();
    await user.click(screen.getByLabelText("Expand tool details"));
    const body = screen.getByTestId("turn-detail");
    Object.defineProperties(body, { scrollHeight: { value: 1000 }, clientHeight: { value: 100 } });
    body.scrollTop = 130;
    fireEvent.scroll(body);
    await user.click(screen.getByText("Read this output"));
    const dialog = screen.getByRole("dialog", { name: "Turn output" });
    expect(within(dialog).getByText("Read this output")).toBeDefined();
    expect(within(dialog).getByTestId("turn-detail").scrollTop).toBe(130);
    expect(within(dialog).getByLabelText("Collapse tool details")).toBeDefined();
    rerender(<TurnDetail turn={{ ...turn, event_count: turn.event_count + 1 }} showThinking />);
    expect(screen.getByRole("dialog", { name: "Turn output" })).toBeDefined();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Turn output" })).toBeNull();
    expect(screen.getByTestId("turn-detail").scrollTop).toBe(130);
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Full screen" }));
  });

  it("opens with the Full screen button on desktop and closes with the modal control", async () => {
    render(<TurnDetail turn={makeTurn({ items: itemsOf(assistantChunk(assistantItems("desktop output"))) })} showThinking />);
    const user = userEvent.setup();
    await user.click(screen.getByText("desktop output"));
    expect(screen.queryByRole("dialog")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Full screen" }));
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("closes only the reader when opened inside an existing modal", async () => {
    const onClose = vi.fn();
    render(<Overlay open modal onClose={onClose} title="Parent turn">
      <TurnDetail turn={makeTurn()} showThinking />
    </Overlay>);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Full screen" }));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Turn output" })).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("opens the reader after a touch hold", () => {
    stubMobileViewport();
    vi.useFakeTimers();
    render(<TurnDetail turn={makeTurn({ items: itemsOf(assistantChunk(assistantItems("hold this output"))) })} showThinking />);
    fireEvent.pointerDown(screen.getByText("hold this output"), { pointerId: 1, pointerType: "touch", button: 0, clientX: 10, clientY: 10 });
    act(() => vi.advanceTimersByTime(500));
    expect(screen.getByRole("dialog", { name: "Turn output" })).toBeDefined();
  });

  it.each(["move", "scroll", "release", "cancel"])("cancels a touch hold on %s", (action) => {
    stubMobileViewport();
    vi.useFakeTimers();
    render(<TurnDetail turn={makeTurn({ items: itemsOf(assistantChunk(assistantItems("scroll this output"))) })} showThinking />);
    const text = screen.getByText("scroll this output");
    fireEvent.pointerDown(text, { pointerId: 1, pointerType: "touch", button: 0, clientX: 10, clientY: 10 });
    if (action === "move") fireEvent.pointerMove(text, { pointerId: 1, clientX: 10, clientY: 40 });
    if (action === "scroll") fireEvent.scroll(screen.getByTestId("turn-detail"));
    if (action === "release") fireEvent.pointerUp(text, { pointerId: 1 });
    if (action === "cancel") fireEvent.pointerCancel(text, { pointerId: 1 });
    act(() => vi.advanceTimersByTime(600));
    if (action === "move") fireEvent.click(text);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("keeps links and selected text from opening the mobile reader", async () => {
    stubMobileViewport();
    const seen: unknown[] = [];
    const unsubscribe = subscribeToAppCommands((command) => seen.push(command));
    render(<TurnDetail turn={makeTurn({ items: itemsOf(assistantChunk(assistantItems("[file](src/lib.rs) and selectable output"))) })}
      showThinking fileTarget={{ repo: "atlas" }} />);
    const user = userEvent.setup();
    await user.click(screen.getByRole("link", { name: "file" }));
    expect(seen).toContainEqual(expect.objectContaining({ type: "open-file", path: "src/lib.rs" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    vi.spyOn(window, "getSelection").mockReturnValue({ toString: () => "selected output" } as Selection);
    fireEvent.click(screen.getByText(/and selectable output/));
    expect(screen.queryByRole("dialog")).toBeNull();
    unsubscribe();
  });

  it("uses a native mobile long press to open the reader instead of the assistant menu", () => {
    stubMobileViewport();
    renderWithContextMenu(<TurnDetail turn={makeTurn({ items: itemsOf(assistantChunk(assistantItems("native hold"))) })} showThinking />);
    fireEvent.contextMenu(screen.getByText("native hold"));
    expect(screen.getByRole("dialog", { name: "Turn output" })).toBeDefined();
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("keeps thinking and context menus above the reader and dismisses them before the reader", async () => {
    renderWithContextMenu(<TurnDetail turn={makeTurn({ items: itemsOf(assistantChunk(assistantItems("inspect output"), ["reasoning text"])) })} showThinking />);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Full screen" }));
    const reader = screen.getByRole("dialog", { name: "Turn output" });
    await user.click(screen.getByRole("button", { name: /thinking/i }));
    expect(reader.contains(screen.getByTestId("thinking-flyout"))).toBe(true);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByTestId("thinking-flyout")).toBeNull();
    expect(screen.getByRole("dialog", { name: "Turn output" })).toBe(reader);
    fireEvent.contextMenu(screen.getByText("inspect output"));
    expect(screen.getByRole("menu")).toBeDefined();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(screen.getByRole("dialog", { name: "Turn output" })).toBe(reader);
  });

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
          items: itemsOf(assistantChunk(assistantItems("before", { tool: "t1" }, "after"))),
        })}
        showThinking={true}
      />,
    );
    expect(screen.getByText(/my prompt/)).toBeDefined();
    expect(screen.getByText(/before/)).toBeDefined();
    expect(screen.getByText("bash")).toBeDefined();
  });

  it("summarizes plan commands from compact metadata before loading their body", () => {
    const pair = makePair({ id: "plan", name: "sulion", operation_type: "sulion_plan", category: "plan", body_loaded: false,
      input: { plan_commands: [{ action: "phase set", phase: "2", status: "completed" }] },
    });
    render(<TurnDetail turn={makeTurn({ tool_pairs: [pair], items: itemsOf(toolChunk("plan")) })} showThinking />);
    const row = screen.getByTestId("tool-pair-row");
    expect(row.className).toContain("td__tool--plan");
    expect(screen.getByText("plan")).toBeDefined();
    expect(screen.getByText("phase set · phase 2 · completed")).toBeDefined();
  });

  it("reports digest failures instead of copying an incomplete streamed turn", async () => {
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    const load = vi.fn().mockRejectedValue(new Error("Digest unavailable"));
    renderWithContextMenu(<TurnDetail turn={makeTurn()} showThinking loadMarkdown={load} />);
    await user.pointer({ keys: "[MouseRight]", target: screen.getByLabelText("Turn actions") });
    await user.click(screen.getByRole("menuitem", { name: "Copy turn as markdown" }));
    expect(await screen.findByText("Digest unavailable")).toBeDefined();
    expect(write).not.toHaveBeenCalled();
  });

  it("labels a filtered prompt distinctly from a genuine orphan turn", () => {
    const { rerender } = renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({ user_prompt_text: "steer left" })}
        showThinking={true}
        hideUserPrompt={true}
      />,
    );
    expect(screen.getByText(/user prompt hidden by filter/)).toBeDefined();
    expect(screen.queryByText(/steer left/)).toBeNull();
    expect(screen.queryByText(/orphan turn/)).toBeNull();

    rerender(
      <>
        <TurnDetail
          turn={makeTurn({ user_prompt_text: "" })}
          showThinking={true}
          hideUserPrompt={true}
        />
        <ContextMenuHost />
      </>,
    );
    expect(screen.getByText(/orphan turn — no user prompt/)).toBeDefined();
  });

  it("renders an archived turn from its markdown digest with a restore hint", () => {
    const archived = {
      ...makeTurn({
        user_prompt_text: "rename the helper",
        markdown:
          "**Prompt**\n\n> rename the helper\n\nRenamed it in `src/widget.rs`.",
        items: [],
      }),
      archived_at: "2026-09-01T00:00:00Z",
      session_uuid: "11111111-2222-3333-4444-555555555555",
    };
    renderWithContextMenu(<TurnDetail turn={archived} showThinking={true} />);
    const banner = screen.getByTestId("turn-archived");
    expect(banner.textContent).toContain("Archived 2026-09-01");
    expect(banner.textContent).toContain(
      "sulion archive restore --session 11111111-2222-3333-4444-555555555555",
    );
    expect(screen.getByText(/Renamed it in/)).toBeDefined();
  });

  it("renders a live turn without the archived banner", () => {
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({ items: itemsOf(assistantChunk(assistantItems("reply"))) })}
        showThinking={true}
      />,
    );
    expect(screen.queryByTestId("turn-archived")).toBeNull();
  });

  it("hides thinking chips when showThinking=false", () => {
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          thinking_count: 1,
          items: itemsOf(assistantChunk(assistantItems("reply"), ["private thought"])),
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
          items: itemsOf(toolChunk("e1")),
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
    });
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          tool_pairs: [pair],
          operation_count: 1,
          items: itemsOf(toolChunk("edit-1")),
        })}
        showThinking={true}
      />,
    );
    // The newest (only) card arrives expanded, so the diff is already
    // visible without a click.
    expect(screen.getByLabelText(/collapse tool details/i)).toBeDefined();
    expect(screen.getByText("before")).toBeDefined();
    expect(screen.getByText("after")).toBeDefined();
  });

  it("marks a failed command apart from a successful edit and tags failed file edits", () => {
    const pair = makePair({
      id: "cell",
      name: "apply_patch",
      category: "create_content",
      input: {
        file_edits: [
          { path: "/repo/a.rs", operation: "update", diff: "-a\n+b", status: "completed" },
          { path: "/repo/b.rs", operation: "add", diff: "+c", status: "failed" },
        ],
        runtime_items: [
          { runtime_item: { type: "FileChange", status: "completed" } },
          { runtime_item: { type: "CommandExecution", status: "failed", exit_code: 101 } },
        ],
      },
      result: { content: "Script completed", is_error: false },
      is_error: false,
    });
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          tool_pairs: [pair],
          operation_count: 1,
          items: itemsOf(toolChunk("cell")),
        })}
        showThinking={true}
      />,
    );
    expect(screen.getByText("command failed")).toBeDefined();
    expect(screen.queryByText("error")).toBeNull();
    expect(screen.getAllByText("failed")).toHaveLength(1);
  });

  it("expands the newest card, collapses it when another arrives, and keeps manual toggles", async () => {
    const first = makePair({ id: "t1", name: "bash", input: { command: "one" } });
    const second = makePair({ id: "t2", name: "bash", input: { command: "two" } });
    const turnWith = (pairs: ReturnType<typeof makePair>[]) =>
      makeTurn({
        tool_pairs: pairs,
        operation_count: pairs.length,
        items: itemsOf(...pairs.map((pair) => toolChunk(pair.id))),
      });

    const { rerender } = renderWithContextMenu(
      <TurnDetail turn={turnWith([first])} showThinking={true} />,
    );
    const rowFor = (id: string) =>
      document.querySelector(`[data-pair-id="${id}"]`) as HTMLElement;
    const expandedIn = (id: string) =>
      rowFor(id).querySelector('[aria-label="Collapse tool details"]') != null;
    expect(expandedIn("t1")).toBe(true);

    // A second card arrives: it expands, the first collapses.
    rerender(
      <>
        <TurnDetail turn={turnWith([first, second])} showThinking={true} />
        <ContextMenuHost />
      </>,
    );
    expect(expandedIn("t1")).toBe(false);
    expect(expandedIn("t2")).toBe(true);

    // Manually expand the first; a further refetch keeps it open.
    const user = userEvent.setup();
    await user.click(
      rowFor("t1").querySelector('[aria-label="Expand tool details"]')!,
    );
    rerender(
      <>
        <TurnDetail turn={turnWith([first, second])} showThinking={true} />
        <ContextMenuHost />
      </>,
    );
    expect(expandedIn("t1")).toBe(true);
    expect(expandedIn("t2")).toBe(true);
  });

  it("starts every mobile tool collapsed, permits tap expansion, and disables hover details", async () => {
    stubMobileViewport();
    const normal = makePair({ id: "t1", name: "read", input: { path: "one.txt" } });
    const error = makePair({
      id: "t2",
      name: "bash",
      input: { command: "false" },
      result: { content: "failed", is_error: true },
      is_error: true,
    });
    const pending = makePair({
      id: "t3",
      name: "edit",
      input: { path: "two.txt" },
      is_pending: true,
    });
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          tool_pairs: [normal, error, pending],
          operation_count: 3,
          items: itemsOf(toolChunk("t1"), toolChunk("t2"), toolChunk("t3")),
        })}
        showThinking={true}
        focusPairId="t1"
        focusKey="mobile-focus"
      />,
    );

    const collapsed = screen.getAllByLabelText("Expand tool details");
    expect(collapsed).toHaveLength(3);
    expect(document.querySelectorAll(".td__tool-body")).toHaveLength(0);

    vi.useFakeTimers();
    fireEvent.mouseEnter(collapsed[0]!);
    vi.advanceTimersByTime(600);
    expect(screen.queryByTestId("tool-hover-card")).toBeNull();
    vi.useRealTimers();

    const user = userEvent.setup();
    await user.click(collapsed[0]!);
    expect(document.querySelectorAll(".td__tool-body")).toHaveLength(1);
  });

  it("shows duration and token usage in the header", () => {
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          duration_ms: 95_000,
          input_tokens: 12_345,
          output_tokens: 678,
        })}
        showThinking={true}
      />,
    );
    expect(screen.getByText("1m35s")).toBeDefined();
    expect(screen.getByText(/↑12\.3k ↓/)).toBeDefined();
  });

  it("prefers the agent's description over the command in collapsed summaries", () => {
    const pair = makePair({
      id: "b1",
      name: "bash",
      input: { command: "cargo test --quiet", description: "Run backend gates" },
    });
    renderWithContextMenu(
      <TurnDetail
        turn={makeTurn({
          tool_pairs: [pair],
          operation_count: 1,
          items: itemsOf(toolChunk("b1")),
        })}
        showThinking={true}
      />,
    );
    // Appears both as the collapsed summary and inside the expanded body.
    const summary = document.querySelector(".td__tool-summary");
    expect(summary?.textContent).toBe("Run backend gates");
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
          items: itemsOf(toolChunk("task-1")),
        })}
        showThinking={true}
        onOpenSubagent={onOpen}
      />,
    );
    await user.click(screen.getByRole("button", { name: "Full screen" }));
    await user.click(screen.getByText(/view agent log/i));
    expect(onOpen).toHaveBeenCalledWith(pair);
    expect(screen.queryByRole("dialog", { name: "Turn output" })).toBeNull();
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
          items: itemsOf(assistantChunk(assistantItems("before"))),
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
          items: itemsOf(toolChunk("tool_focus"), toolChunk("tool_other")),
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
