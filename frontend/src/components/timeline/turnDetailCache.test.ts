import { describe, expect, it } from "vitest";
import type { TimelineAssistantItem, TimelineItem } from "../../api/types";
import { createItemGrouper, groupItems } from "./turnDetailCache";

function assistant(offset: number, ...entries: Array<string | { tool: string }>): TimelineItem {
  const items: TimelineAssistantItem[] = entries.map((entry) =>
    typeof entry === "string" ? { kind: "text", text: entry } : { kind: "tool", pair_id: entry.tool },
  );
  return { offset, kind: "assistant", items, thinking: [] };
}

function system(offset: number): TimelineItem {
  return { offset, kind: "system", subtype: "note", text: "note", is_meta: false };
}

describe("groupItems", () => {
  it("bounds text-only groups and preserves full groups across later batches", () => {
    const group = createItemGrouper();
    const items = Array.from({ length: 1024 }, (_, i) => assistant(i, `message ${i}`));
    const first = group(items);
    expect(first).toHaveLength(16);
    const next = group([...items, assistant(1024, "next")]);
    expect(next).toHaveLength(17);
    for (let i = 0; i < first.length; i++) expect(next[i]).toBe(first[i]);
  });
  it("retains closed block identities and only joins the trailing block on append", () => {
    const group = createItemGrouper();
    const first = [assistant(1, "closed", { tool: "a" }), assistant(2, "open")];
    const before = group(first);
    const tail = assistant(3, "tail");
    const after = group([...first, tail]);
    expect(after[0]).toBe(before[0]);
    expect(after[1]).toBe(before[1]);
    expect(after[2]).not.toBe(before[2]);
    expect(after).toEqual(groupItems([...first, assistant(3, "tail")]));
    expect(group([...first, tail])).toBe(after);
  });
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
