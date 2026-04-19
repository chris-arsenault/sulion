import { describe, expect, it } from "vitest";

import type { TimelineBlock, TimelineEvent } from "../../api/types";
import {
  formatAssistantEvent,
  formatAssistantText,
  formatToolPair,
  formatTurn,
} from "./markdown-export";
import type { ToolPair, Turn } from "./grouping";
import { makeEvent, textBlock, thinkingBlock, toolUseBlock } from "./test-helpers";

let offset = 0;
function mk(kind: string, overrides: Parameters<typeof makeEvent>[1]) {
  offset += 100;
  return makeEvent(kind, {
    byte_offset: offset,
    timestamp: "2025-01-01T00:00:00Z",
    ...overrides,
  });
}
function mkUserPrompt(text: string) {
  return mk("user", { blocks: [textBlock(0, text)] });
}
function mkAssistant(blocks: TimelineBlock[]) {
  return mk("assistant", { blocks });
}

function turnFrom(prompt: TimelineEvent, events: TimelineEvent[], pairs: ToolPair[]): Turn {
  return {
    id: prompt.byte_offset,
    userPrompt: prompt,
    events: [prompt, ...events],
    startTimestamp: prompt.timestamp,
    endTimestamp: events[events.length - 1]?.timestamp ?? prompt.timestamp,
    durationMs: 0,
    toolPairs: pairs,
    thinkingCount: 0,
    hasErrors: pairs.some((p) => p.isError),
  };
}

describe("markdown-export", () => {
  describe("formatAssistantText", () => {
    it("returns plain text content joined by blank lines", () => {
      const e = mkAssistant([
        textBlock(0, "First paragraph."),
        textBlock(1, "Second paragraph."),
      ]);
      expect(formatAssistantText(e)).toBe("First paragraph.\n\nSecond paragraph.");
    });

    it("ignores tool_use and thinking blocks", () => {
      const e = mkAssistant([
        textBlock(0, "reply text"),
        toolUseBlock(1, "t", "read", {}, "Read"),
        thinkingBlock(2, "noise"),
      ]);
      expect(formatAssistantText(e)).toBe("reply text");
    });
  });

  describe("formatToolPair", () => {
    const mkPair = (overrides: Partial<ToolPair>): ToolPair => ({
      id: "t",
      name: "read",
      input: {},
      use: { type: "tool_use", id: "t", name: "read" } as never,
      useEvent: mkAssistant([]),
      result: null,
      resultEvent: null,
      isError: false,
      isPending: false,
      ...overrides,
    });

    it("Bash renders as ```bash fence", () => {
      const out = formatToolPair(
        mkPair({ name: "bash", input: { command: "ls -la" } }),
      );
      expect(out).toContain("```bash");
      expect(out).toContain("ls -la");
      expect(out).toContain("**Tool:** `bash`");
    });

    it("Edit renders as ```diff fence with -/+ lines", () => {
      const out = formatToolPair(
        mkPair({
          name: "edit",
          input: {
            path: "/src/foo.ts",
            old_text: "hello",
            new_text: "hello world",
          },
        }),
      );
      expect(out).toContain("/src/foo.ts");
      expect(out).toContain("```diff");
      expect(out).toContain("- hello");
      expect(out).toContain("+ hello world");
    });

    it("TodoWrite renders as a markdown task list", () => {
      const out = formatToolPair(
        mkPair({
          name: "todo_write",
          input: {
            todos: [
              { status: "completed", content: "done thing" },
              { status: "in_progress", content: "doing thing" },
              { status: "pending", content: "future thing" },
            ],
          },
        }),
      );
      expect(out).toContain("- [x] done thing");
      expect(out).toContain("- [~] doing thing");
      expect(out).toContain("- [ ] future thing");
    });

    it("Error pair adds '(error)' marker to header", () => {
      const out = formatToolPair(
        mkPair({
          name: "bash",
          input: { command: "oops" },
          result: {
            type: "tool_result",
            tool_use_id: "t",
            content: "command not found",
            is_error: true,
          },
          isError: true,
        }),
      );
      expect(out).toContain("_(error)_");
      expect(out).toContain("command not found");
    });

    it("Pending pair marks pending in header and omits result", () => {
      const out = formatToolPair(
        mkPair({ name: "bash", input: { command: "ls" }, isPending: true }),
      );
      expect(out).toContain("_(pending)_");
      expect(out).not.toContain("Result");
    });

    it("Body containing triple-backticks escapes the fence to four", () => {
      const out = formatToolPair(
        mkPair({
          name: "bash",
          input: { command: "echo '```'" },
        }),
      );
      expect(out).toContain("````bash");
    });
  });

  describe("formatTurn", () => {
    it("renders prompt + assistant text + tool calls as a single markdown doc", () => {
      offset = 0;
      const prompt = mkUserPrompt("edit foo.ts");
      const asst = mkAssistant([
        textBlock(0, "I'll do that."),
        toolUseBlock(1, "t1", "edit", { path: "/foo", old_text: "a", new_text: "b" }, "Edit"),
        textBlock(2, "Done."),
      ]);
      const pair: ToolPair = {
        id: "t1",
        name: "edit",
        input: { path: "/foo", old_text: "a", new_text: "b" },
        use: { type: "tool_use", id: "t1", name: "edit" } as never,
        useEvent: asst,
        result: { type: "tool_result", tool_use_id: "t1", content: "done" },
        resultEvent: asst,
        isError: false,
        isPending: false,
      };
      const t = turnFrom(prompt, [asst], [pair]);
      const out = formatTurn(t);
      expect(out).toContain("**Prompt**");
      expect(out).toContain("> edit foo.ts");
      expect(out).toContain("I'll do that.");
      expect(out).toContain("**Tool:** `edit`");
      expect(out).toContain("- a");
      expect(out).toContain("+ b");
      expect(out).toContain("Done.");
    });

    it("multi-line prompt gets quoted on each line", () => {
      offset = 0;
      const prompt = mkUserPrompt("line one\nline two");
      const t = turnFrom(prompt, [], []);
      const out = formatTurn(t);
      expect(out).toContain("> line one");
      expect(out).toContain("> line two");
    });

    it("orphan turn (no user prompt) still formats assistant content", () => {
      offset = 0;
      const asst = mkAssistant([textBlock(0, "boot")]);
      const t: Turn = {
        id: asst.byte_offset,
        userPrompt: null,
        events: [asst],
        startTimestamp: asst.timestamp,
        endTimestamp: asst.timestamp,
        durationMs: 0,
        toolPairs: [],
        thinkingCount: 0,
        hasErrors: false,
      };
      const out = formatTurn(t);
      expect(out).toContain("boot");
    });
  });

  describe("formatAssistantEvent", () => {
    it("interleaves text and tool calls in block order", () => {
      offset = 0;
      const asst = mkAssistant([
        textBlock(0, "first"),
        toolUseBlock(1, "t", "bash", { command: "ls" }, "Bash"),
        textBlock(2, "second"),
      ]);
      const pair: ToolPair = {
        id: "t",
        name: "bash",
        input: { command: "ls" },
        use: { type: "tool_use", id: "t", name: "bash" } as never,
        useEvent: asst,
        result: null,
        resultEvent: null,
        isError: false,
        isPending: true,
      };
      const out = formatAssistantEvent(asst, new Map([["t", pair]]));
      const firstIdx = out.indexOf("first");
      const toolIdx = out.indexOf("**Tool:**");
      const secondIdx = out.indexOf("second");
      expect(firstIdx).toBeGreaterThanOrEqual(0);
      expect(toolIdx).toBeGreaterThan(firstIdx);
      expect(secondIdx).toBeGreaterThan(toolIdx);
    });
  });
});
