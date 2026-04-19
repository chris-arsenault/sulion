import { afterEach, describe, expect, it } from "vitest";
import { act, renderHook } from "@testing-library/react";

import type { TimelineEvent } from "../../api/types";
import { groupIntoTurns, prefilter, type ToolPair, type Turn } from "./grouping";
import {
  DEFAULT_FILTERS,
  eventIsVisible,
  hasActiveIncludeFilters,
  toolPairIsVisible,
  turnPassesIncludeFilters,
  useTimelineFilters,
  type TimelineFilters,
} from "./filters";
import { makeEvent, textBlock, toolResultBlock, toolUseBlock } from "./test-helpers";

let offset = 0;
function mk(kind: string, overrides: Parameters<typeof makeEvent>[1] = {}) {
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
function mkAssistantText(text: string) {
  return mk("assistant", { blocks: [textBlock(0, text)] });
}
function mkAssistantTool(
  id: string,
  name: string,
  input: Record<string, unknown> = {},
) {
  return mk("assistant", { blocks: [toolUseBlock(0, id, name, input)] });
}
function mkToolResult(
  tool_use_id: string,
  content: string,
  is_error = false,
) {
  return mk("user", { blocks: [toolResultBlock(0, tool_use_id, content, is_error)] });
}

function buildTurn(events: TimelineEvent[]): Turn {
  const turns = groupIntoTurns(
    prefilter(events, { showBookkeeping: false, showSidechain: false }),
  );
  expect(turns.length).toBeGreaterThan(0);
  return turns[0]!;
}

function base(overrides: Partial<TimelineFilters> = {}): TimelineFilters {
  return {
    hiddenSpeakers: new Set(),
    hiddenTools: new Set(),
    errorsOnly: false,
    showThinking: true,
    showBookkeeping: false,
    showSidechain: false,
    filePath: "",
    ...overrides,
  };
}

function pair(name: string): ToolPair {
  return {
    id: `id-${name}`,
    name,
    input: {},
    use: { type: "tool_use", id: `id-${name}`, name } as never,
    useEvent: mkAssistantText(""),
    result: null,
    resultEvent: null,
    isError: false,
    isPending: false,
  };
}

describe("hide semantics — simple and obvious", () => {
  describe("eventIsVisible", () => {
    it("returns true for any event when nothing is hidden (default)", () => {
      expect(eventIsVisible(mkUserPrompt("hi"), base())).toBe(true);
      expect(eventIsVisible(mkAssistantText("hey"), base())).toBe(true);
      expect(eventIsVisible(mkToolResult("t", "ok"), base())).toBe(true);
    });

    it("hides user events when 'user' is in hiddenSpeakers", () => {
      const f = base({ hiddenSpeakers: new Set(["user"]) });
      expect(eventIsVisible(mkUserPrompt("hi"), f)).toBe(false);
      expect(eventIsVisible(mkAssistantText("hey"), f)).toBe(true);
      expect(eventIsVisible(mkToolResult("t", "ok"), f)).toBe(true);
    });

    it("hides assistant events when 'assistant' is hidden", () => {
      const f = base({ hiddenSpeakers: new Set(["assistant"]) });
      expect(eventIsVisible(mkAssistantText("hey"), f)).toBe(false);
      expect(eventIsVisible(mkUserPrompt("hi"), f)).toBe(true);
    });

    it("hides tool_result wrapper events when 'tool_result' is hidden", () => {
      const f = base({ hiddenSpeakers: new Set(["tool_result"]) });
      expect(eventIsVisible(mkToolResult("t", "ok"), f)).toBe(false);
      expect(eventIsVisible(mkUserPrompt("hi"), f)).toBe(true);
    });
  });

  describe("toolPairIsVisible", () => {
    it("returns true when the tool name is not hidden", () => {
      expect(toolPairIsVisible(pair("edit"), base())).toBe(true);
    });

    it("hides the pair when its tool name is in hiddenTools", () => {
      const f = base({ hiddenTools: new Set(["edit"]) });
      expect(toolPairIsVisible(pair("edit"), f)).toBe(false);
      expect(toolPairIsVisible(pair("bash"), f)).toBe(true);
    });

    it("user-reported expectation: click Edit → Edit pair hidden, other pairs still visible", () => {
      const f = base({ hiddenTools: new Set(["edit"]) });
      expect(toolPairIsVisible(pair("edit"), f)).toBe(false);
      expect(toolPairIsVisible(pair("bash"), f)).toBe(true);
      expect(toolPairIsVisible(pair("read"), f)).toBe(true);
    });

    it("hiding multiple tools hides all of them, leaves others alone", () => {
      const f = base({ hiddenTools: new Set(["edit", "bash"]) });
      expect(toolPairIsVisible(pair("edit"), f)).toBe(false);
      expect(toolPairIsVisible(pair("bash"), f)).toBe(false);
      expect(toolPairIsVisible(pair("read"), f)).toBe(true);
    });
  });

  describe("turnPassesIncludeFilters — only errorsOnly and filePath drop turns", () => {
    function canonicalTurn(): Turn {
      offset = 0;
      return buildTurn([
        mkUserPrompt("edit foo.ts"),
        mkAssistantTool("t1", "edit", { path: "/src/foo.ts" }),
        mkToolResult("t1", "edit applied"),
        mkAssistantText("done"),
      ]);
    }

    it("default state: every turn passes", () => {
      expect(turnPassesIncludeFilters(canonicalTurn(), base())).toBe(true);
    });

    it("hiding a speaker does NOT drop the turn from the list — only hides content inside", () => {
      const f = base({ hiddenSpeakers: new Set(["user"]) });
      expect(turnPassesIncludeFilters(canonicalTurn(), f)).toBe(true);
    });

    it("hiding a tool does NOT drop the turn from the list — only hides the pair row", () => {
      const f = base({ hiddenTools: new Set(["edit"]) });
      expect(turnPassesIncludeFilters(canonicalTurn(), f)).toBe(true);
    });

    it("errorsOnly drops turns without errors", () => {
      offset = 0;
      const errTurn = buildTurn([
        mkUserPrompt("p"),
        mkAssistantTool("e", "bash", { command: "fail" }),
        mkToolResult("e", "err", true),
      ]);
      const okTurn = canonicalTurn();
      expect(turnPassesIncludeFilters(errTurn, base({ errorsOnly: true }))).toBe(true);
      expect(turnPassesIncludeFilters(okTurn, base({ errorsOnly: true }))).toBe(false);
    });

    it("filePath drops turns with no matching file", () => {
      offset = 0;
      const fooTurn = buildTurn([
        mkUserPrompt("p"),
        mkAssistantTool("r", "read", { path: "/src/foo.ts" }),
        mkToolResult("r", "content"),
      ]);
      expect(turnPassesIncludeFilters(fooTurn, base({ filePath: "foo" }))).toBe(true);
      expect(turnPassesIncludeFilters(fooTurn, base({ filePath: "bar" }))).toBe(false);
    });
  });

  describe("hasActiveIncludeFilters", () => {
    it("is false by default", () => {
      expect(hasActiveIncludeFilters(base())).toBe(false);
    });

    it("is true when errorsOnly is on", () => {
      expect(hasActiveIncludeFilters(base({ errorsOnly: true }))).toBe(true);
    });

    it("is true when filePath is non-empty", () => {
      expect(hasActiveIncludeFilters(base({ filePath: "x" }))).toBe(true);
    });

    it("is false when only hide-filters are active (those don't drop turns)", () => {
      expect(
        hasActiveIncludeFilters(base({ hiddenSpeakers: new Set(["user"]) })),
      ).toBe(false);
      expect(
        hasActiveIncludeFilters(base({ hiddenTools: new Set(["edit"]) })),
      ).toBe(false);
      expect(
        hasActiveIncludeFilters(base({ showThinking: false })),
      ).toBe(false);
    });
  });
});

describe("useTimelineFilters", () => {
  afterEach(() => {
    window.localStorage.clear();
  });

  it("returns defaults on first render", () => {
    const { result } = renderHook(() => useTimelineFilters());
    expect(result.current.filters).toEqual(DEFAULT_FILTERS);
  });

  it("toggleSpeaker flips the hidden state and persists", () => {
    const { result } = renderHook(() => useTimelineFilters());
    expect(result.current.filters.hiddenSpeakers.has("user")).toBe(false);
    act(() => result.current.toggleSpeaker("user"));
    expect(result.current.filters.hiddenSpeakers.has("user")).toBe(true);
    act(() => result.current.toggleSpeaker("user"));
    expect(result.current.filters.hiddenSpeakers.has("user")).toBe(false);

    act(() => result.current.toggleSpeaker("assistant"));
    const { result: r2 } = renderHook(() => useTimelineFilters());
    expect(r2.current.filters.hiddenSpeakers.has("assistant")).toBe(true);
  });

  it("toggleTool adds and removes the tool from hiddenTools", () => {
    const { result } = renderHook(() => useTimelineFilters());
    act(() => result.current.toggleTool("edit"));
    expect(result.current.filters.hiddenTools.has("edit")).toBe(true);
    act(() => result.current.toggleTool("edit"));
    expect(result.current.filters.hiddenTools.has("edit")).toBe(false);
  });

  it("reset returns to defaults", () => {
    const { result } = renderHook(() => useTimelineFilters());
    act(() => {
      result.current.toggleSpeaker("user");
      result.current.toggleTool("edit");
      result.current.setShowThinking(false);
      result.current.setFilePath("foo");
    });
    act(() => result.current.reset());
    expect(result.current.filters).toEqual(DEFAULT_FILTERS);
  });

  it("defaults are strict booleans (no undefined for toggles)", () => {
    const { result } = renderHook(() => useTimelineFilters());
    const f = result.current.filters;
    expect(typeof f.errorsOnly).toBe("boolean");
    expect(typeof f.showThinking).toBe("boolean");
    expect(typeof f.showBookkeeping).toBe("boolean");
    expect(typeof f.showSidechain).toBe("boolean");
    expect(typeof f.filePath).toBe("string");
    expect(f.hiddenSpeakers).toBeInstanceOf(Set);
    expect(f.hiddenTools).toBeInstanceOf(Set);
  });

  it("localStorage rehydration coerces garbage values to strict booleans", () => {
    window.localStorage.setItem(
      "shuttlecraft.timeline.filters.v2",
      JSON.stringify({
        hiddenSpeakers: ["user"],
        hiddenTools: ["edit"],
        errorsOnly: "maybe",
        showThinking: null,
        showBookkeeping: undefined,
        showSidechain: 1,
        filePath: 42,
      }),
    );
    const { result } = renderHook(() => useTimelineFilters());
    const f = result.current.filters;
    expect(typeof f.errorsOnly).toBe("boolean");
    expect(typeof f.showThinking).toBe("boolean");
    expect(typeof f.showBookkeeping).toBe("boolean");
    expect(typeof f.showSidechain).toBe("boolean");
    expect(typeof f.filePath).toBe("string");
    expect(f.hiddenSpeakers.has("user")).toBe(true);
    expect(f.hiddenTools.has("edit")).toBe(true);
  });
});
