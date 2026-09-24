import { describe, expect, it } from "vitest";

import type {
  TimelineAssistantItem,
  TimelineItem,
  TimelineToolPair,
  TimelineTurnDetailResponse,
} from "../../api/types";
import { applyTurnDetail, groupItems } from "./turnDetailCache";
import type { Turn } from "./grouping";

function pair(id: string, pending: boolean): TimelineToolPair {
  return {
    id,
    name: "bash",
    is_error: false,
    is_pending: pending,
    file_touches: [],
  };
}

function assistant(offset: number, ...entries: Array<string | { tool: string }>): TimelineItem {
  const items: TimelineAssistantItem[] = entries.map((entry) =>
    typeof entry === "string" ? { kind: "text", text: entry } : { kind: "tool", pair_id: entry.tool },
  );
  return { offset, kind: "assistant", items, thinking: [] };
}

function system(offset: number): TimelineItem {
  return { offset, kind: "system", subtype: "note", text: "note", is_meta: false };
}

function turn(items: TimelineItem[], pairs: TimelineToolPair[], markdown = ""): Turn {
  return {
    id: 1,
    preview: "prompt",
    start_timestamp: "2026-09-24T10:00:00Z",
    end_timestamp: "2026-09-24T10:00:05Z",
    duration_ms: 5000,
    event_count: items.length + 1,
    operation_count: pairs.length,
    tool_pairs: pairs,
    thinking_count: 0,
    has_errors: false,
    markdown,
    items,
  };
}

function response(body: Turn, through: number, since?: number): TimelineTurnDetailResponse {
  return {
    session_uuid: "s",
    session_agent: "claude-code",
    turn: body,
    through,
    since: since ?? null,
  };
}

describe("applyTurnDetail", () => {
  const whole = applyTurnDetail(
    undefined,
    response(turn([assistant(10, "one", { tool: "a" })], [pair("a", true)], "digest"), 20),
  )!;

  it("keeps a whole read as is", () => {
    expect(whole.through).toBe(20);
    expect(whole.turn.items.map((item) => item.offset)).toEqual([10]);
    expect(whole.turn.markdown).toBe("digest");
  });

  it("appends new items and replaces changed pairs", () => {
    const delta = response(
      turn([assistant(30, { tool: "b" }), assistant(40, "two")], [pair("a", false), pair("b", true)]),
      40,
      20,
    );
    const merged = applyTurnDetail(whole, delta)!;
    expect(merged.through).toBe(40);
    expect(merged.turn.items.map((item) => item.offset)).toEqual([10, 30, 40]);
    expect(merged.turn.tool_pairs.map((p) => [p.id, p.is_pending])).toEqual([
      ["a", false],
      ["b", true],
    ]);
    expect(merged.turn.markdown).toBe("digest");
  });

  it("ignores an item it already has", () => {
    const delta = response(turn([assistant(10, "one", { tool: "a" })], []), 20, 20);
    expect(applyTurnDetail(whole, delta)!.turn.items).toHaveLength(1);
  });

  it("refuses a delta asked against another base", () => {
    const delta = response(turn([], []), 90, 70);
    expect(applyTurnDetail(whole, delta)).toBeNull();
    expect(applyTurnDetail(undefined, delta)).toBeNull();
  });
});

describe("groupItems", () => {
  it("joins assistant text until a call, then lists the calls as rows", () => {
    const blocks = groupItems([
      assistant(1, "one"),
      assistant(2, "two", { tool: "a" }, { tool: "b" }),
      assistant(3, "three"),
      system(4),
      assistant(5, "four"),
    ]);
    expect(blocks).toEqual([
      {
        kind: "assistant",
        items: [
          { kind: "text", text: "one" },
          { kind: "text", text: "two" },
          { kind: "tool", pair_id: "a" },
          { kind: "tool", pair_id: "b" },
        ],
        thinking: [],
      },
      { kind: "tool", pair_id: "a" },
      { kind: "tool", pair_id: "b" },
      { kind: "assistant", items: [{ kind: "text", text: "three" }], thinking: [] },
      { kind: "system", subtype: "note", text: "note", is_meta: false },
      { kind: "assistant", items: [{ kind: "text", text: "four" }], thinking: [] },
    ]);
  });

  it("leaves out a block that only made calls", () => {
    expect(groupItems([assistant(1, { tool: "a" })])).toEqual([{ kind: "tool", pair_id: "a" }]);
  });
});
