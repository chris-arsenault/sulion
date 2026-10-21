import { describe, expect, it } from "vitest";
import { filterTurn } from "./filterTurn";
import { DEFAULT_FILTERS } from "./filters";
import { makePair, makeTurn, itemsOf, assistantChunk, assistantItems } from "./test-helpers";

describe("local turn visibility", () => {
  it("hides plan calls independently while keeping mixed utility calls", () => {
    const pure = makePair({ id: "plan", category: "plan" });
    const mixed = makePair({ id: "mixed", category: "utility", input: { plan_commands: [{ action: "current" }] } });
    const turn = makeTurn({ tool_pairs: [pure, mixed], items: itemsOf(assistantChunk(assistantItems({ tool: "plan" }, { tool: "mixed" }))) });
    const result = filterTurn(turn, { ...DEFAULT_FILTERS, hiddenOperationCategories: new Set(["plan" as const]) });
    expect(result.items).toMatchObject([{ items: [{ kind: "tool", pair_id: "mixed" }] }]);
    expect(turn.items[0]).toMatchObject({ items: [{ pair_id: "plan" }, { pair_id: "mixed" }] });
  });
  it("hides operation categories while preserving text and the original cached records", () => {
    const pair = makePair({ id: "edit", category: "create_content", result: { content: "done", is_error: false } });
    const turn = makeTurn({ tool_pairs: [pair], items: itemsOf(assistantChunk(assistantItems("reply", { tool: "edit" }))) });
    const filters = { ...DEFAULT_FILTERS, hiddenOperationCategories: new Set(["create_content" as const]) };
    const first = filterTurn(turn, filters);
    expect(first.items).toMatchObject([{ items: [{ kind: "text", text: "reply" }] }]);
    expect(filterTurn(turn, filters).items[0]).toBe(first.items[0]);
    expect(filterTurn(turn, DEFAULT_FILTERS).items[0]).toBe(turn.items[0]);
    expect(filterTurn(turn, { ...DEFAULT_FILTERS, hiddenSpeakers: new Set(["assistant" as const]) }).items).toEqual([]);
    const withoutResults = filterTurn(turn, { ...DEFAULT_FILTERS, hiddenSpeakers: new Set(["tool_result" as const]) });
    expect(withoutResults.tool_pairs[0]?.result).toBeNull();
    expect(turn.tool_pairs[0]?.result?.content).toBe("done");
  });

  it("keeps agent messages while hiding bookkeeping telemetry", () => {
    const turn = makeTurn({ items: [
      { offset: 1, kind: "system", subtype: "agent_message", text: "message", is_meta: false },
      { offset: 2, kind: "system", subtype: "turn_duration", text: "telemetry", is_meta: false },
      { offset: 3, kind: "system", subtype: "notice", text: "meta", is_meta: true },
    ] });
    expect(filterTurn(turn, DEFAULT_FILTERS).items.map((item) => item.offset)).toEqual([1]);
    expect(filterTurn(turn, { ...DEFAULT_FILTERS, showBookkeeping: true }).items).toEqual(turn.items);
  });

  it("hides API and collaboration metadata while keeping agent communication", () => {
    const details = { event_uuid: null, parent_event_uuid: null, related_tool_use_id: null,
      subtype: null, speaker: "other", content_kind: "none", blocks: [] };
    const turn = makeTurn({ items: [
      { offset: 1, kind: "generic", label: "api-request", details },
      { offset: 2, kind: "generic", label: "api-request-blob", details },
      { offset: 3, kind: "generic", label: "inter_agent_communication_metadata", details },
      { offset: 4, kind: "agent_message", author: "/root/worker", recipient: "/root", message_type: "MESSAGE", text: "hello" },
      { offset: 5, kind: "generic", label: "unknown-future-event", details },
    ] });
    expect(filterTurn(turn, DEFAULT_FILTERS).items.map((item) => item.offset)).toEqual([4, 5]);
    expect(filterTurn(turn, { ...DEFAULT_FILTERS, showBookkeeping: true }).items).toEqual(turn.items);
  });
});
