import { describe, expect, it } from "vitest";

import { groupIntoTurns, prefilter } from "./grouping";
import { makeEvent, textBlock, thinkingBlock, toolResultBlock, toolUseBlock } from "./test-helpers";

let nextOffset = 0;
function mk(kind: string, overrides: Parameters<typeof makeEvent>[1] = {}, timestamp = "2025-01-01T00:00:00Z") {
  nextOffset += 100;
  return makeEvent(kind, { byte_offset: nextOffset, timestamp, ...overrides });
}

function userPrompt(text: string, ts?: string) {
  return mk("user", { blocks: [textBlock(0, text)] }, ts);
}

function toolResultUser(toolUseId: string, content: string, is_error = false) {
  return mk("user", { blocks: [toolResultBlock(0, toolUseId, content, is_error)] });
}

function assistant(blocks: ReturnType<typeof textBlock>[], ts?: string) {
  return mk("assistant", { blocks }, ts);
}

describe("groupIntoTurns", () => {
  it("starts a new turn on each real user prompt", () => {
    nextOffset = 0;
    const events = [
      userPrompt("hello"),
      assistant([textBlock(0, "hi")]),
      userPrompt("second"),
      assistant([textBlock(0, "ok")]),
    ];
    const turns = groupIntoTurns(events);
    expect(turns).toHaveLength(2);
    expect(turns[0]!.events).toHaveLength(2);
    expect(turns[1]!.events).toHaveLength(2);
  });

  it("folds tool_result user events into the containing turn without starting a new one", () => {
    nextOffset = 0;
    const events = [
      userPrompt("read a file"),
      assistant([
        toolUseBlock(0, "t1", "read", { path: "/a.txt" }, "Read"),
      ]),
      toolResultUser("t1", "file contents"),
      assistant([textBlock(0, "done")]),
      userPrompt("next"),
    ];
    const turns = groupIntoTurns(events);
    expect(turns).toHaveLength(2);
    expect(turns[0]!.events).toHaveLength(4); // prompt + assistant + tool_result + assistant
    expect(turns[1]!.events).toHaveLength(1);
  });

  it("pairs tool_use with tool_result by id and carries error/pending state", () => {
    nextOffset = 0;
    const events = [
      userPrompt("prompt"),
      assistant([
        toolUseBlock(0, "ok1", "read", { path: "/a" }, "Read"),
        toolUseBlock(1, "err1", "bash", { command: "bad" }, "Bash"),
        toolUseBlock(2, "pend1", "grep", { pattern: "x" }, "Grep"),
      ]),
      toolResultUser("ok1", "ok content"),
      toolResultUser("err1", "stderr text", true),
      // pend1 intentionally has no result
    ];
    const [turn] = groupIntoTurns(events);
    expect(turn!.toolPairs).toHaveLength(3);
    const byId = new Map(turn!.toolPairs.map((p) => [p.id, p]));
    expect(byId.get("ok1")!.isPending).toBe(false);
    expect(byId.get("ok1")!.isError).toBe(false);
    expect(byId.get("err1")!.isError).toBe(true);
    expect(byId.get("pend1")!.isPending).toBe(true);
    expect(turn!.hasErrors).toBe(true);
  });

  it("counts thinking-block assistant events", () => {
    nextOffset = 0;
    const events = [
      userPrompt("p"),
      assistant([thinkingBlock(0, "reasoning 1")]),
      assistant([textBlock(0, "hi")]),
      assistant([
        thinkingBlock(0, "reasoning 2"),
        textBlock(1, "done"),
      ]),
    ];
    const [turn] = groupIntoTurns(events);
    expect(turn!.thinkingCount).toBe(2);
  });

  it("excludes signature-only (empty thinking) events from the count", () => {
    nextOffset = 0;
    const events = [
      userPrompt("p"),
      assistant([thinkingBlock(0, "")]),
      assistant([thinkingBlock(0, "   ")]),
      assistant([thinkingBlock(0, "real content")]),
    ];
    const [turn] = groupIntoTurns(events);
    expect(turn!.thinkingCount).toBe(1);
  });

  it("creates a synthetic orphan turn for events before any user prompt", () => {
    nextOffset = 0;
    const events = [
      assistant([textBlock(0, "boot")]),
      // first real prompt opens a second turn
      userPrompt("first real prompt"),
    ];
    const turns = groupIntoTurns(events);
    expect(turns).toHaveLength(2);
    expect(turns[0]!.userPrompt).toBeNull();
    expect(turns[1]!.userPrompt).not.toBeNull();
  });

  it("computes duration from first to last event", () => {
    nextOffset = 0;
    const events = [
      userPrompt("p", "2025-01-01T00:00:00Z"),
      assistant([textBlock(0, "t")], "2025-01-01T00:00:05Z"),
    ];
    const [turn] = groupIntoTurns(events);
    expect(turn!.durationMs).toBe(5000);
  });
});

describe("prefilter", () => {
  it("drops bookkeeping event kinds when showBookkeeping=false", () => {
    nextOffset = 0;
    const events = [
      userPrompt("p"),
      mk("file-history-snapshot"),
      mk("permission-mode"),
      mk("last-prompt"),
      mk("queue-operation"),
      mk("attachment"),
    ];
    const out = prefilter(events, { showBookkeeping: false, showSidechain: false });
    expect(out).toHaveLength(1);
    expect(out[0]!.kind).toBe("user");
  });

  it("keeps bookkeeping event kinds when showBookkeeping=true", () => {
    nextOffset = 0;
    const events = [
      userPrompt("p"),
      mk("file-history-snapshot"),
    ];
    const out = prefilter(events, { showBookkeeping: true, showSidechain: false });
    expect(out).toHaveLength(2);
  });

  it("drops sidechain events when showSidechain=false", () => {
    nextOffset = 0;
    const events = [
      userPrompt("p"),
      mk("assistant", {
        is_sidechain: true,
        blocks: [textBlock(0, "sub")],
      }),
    ];
    const out = prefilter(events, { showBookkeeping: false, showSidechain: false });
    expect(out).toHaveLength(1);
  });

  it("drops isMeta system events when showBookkeeping=false", () => {
    nextOffset = 0;
    const events = [
      userPrompt("p"),
      mk("system", { is_meta: true }),
      mk("system", { is_meta: false, blocks: [textBlock(0, "real system")] }),
    ];
    const out = prefilter(events, { showBookkeeping: false, showSidechain: false });
    expect(out).toHaveLength(2); // user + non-meta system
    expect(out.map((e) => e.kind)).toEqual(["user", "system"]);
  });
});
