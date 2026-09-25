import { afterEach, describe, expect, it, vi } from "vitest";
import { waitFor } from "@testing-library/react";
import * as turnStream from "../api/turnStream";
import type { TurnCursor, TurnRecord } from "../api/turnStream";
import { applyStreamRecord, hydrateOperation, mergeOperations, refreshTurn, retainTurn, useTurnDetailStore } from "./TurnDetailStore";
import { makeTurn } from "../components/timeline/test-helpers";

afterEach(() => vi.restoreAllMocks());
const cursor: TurnCursor = { turn_id: 1, generation: "one", since: -1, through: 10, item_after: 1, operation_after: -1, items_done: false };
const header: TurnRecord = { kind: "header", turn: makeTurn(), cursor, reset: true, archived_at: null };
const item = { kind: "assistant" as const, offset: 1, items: [{ kind: "text" as const, text: "first" }], thinking: [] };

describe("stream cache", () => {
  it("rejects late body responses after newer metadata or a generation reset", async () => {
    let finish!: (bodies: turnStream.OperationBody[]) => void;
    vi.spyOn(turnStream, "getOperationBodies").mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    const pair = { id: "p", name: "bash", is_error: false, is_pending: false, file_touches: [],
      body_version: 20, body_loaded: false, input: {}, result: null };
    const entry = { revision: 1, loading: false, turn: makeTurn({ generation: "new", tool_pairs: [pair] }) };
    useTurnDetailStore.setState({ entries: new Map([["s:1", entry]]) });
    const read = hydrateOperation("s", 1, "p", new AbortController().signal);
    await waitFor(() => expect(finish).toBeDefined());
    finish([{ id: "p", generation: "old", body_version: 30, input: { command: "obsolete" }, result: null }]);
    await read;
    expect(useTurnDetailStore.getState().entries.get("s:1")).toBe(entry);

    const stale = hydrateOperation("s", 1, "p", new AbortController().signal);
    await new Promise((resolve) => setTimeout(resolve, 5));
    finish([{ id: "p", generation: "new", body_version: 19, input: { command: "older" }, result: null }]);
    await stale;
    expect(useTurnDetailStore.getState().entries.get("s:1")?.turn?.tool_pairs[0]).toBe(pair);
  });

  it("evicts inactive turns while retaining an observed turn", async () => {
    vi.spyOn(turnStream, "readTurnStream").mockImplementation(async (_session, turn, _cursor, _signal, receive) => {
      receive({ ...header, turn: makeTurn({ id: turn }) });
      receive({ kind: "complete", cursor: { ...cursor, through: null } });
    });
    const keep = retainTurn("s", 0);
    refreshTurn("s", 0, 1);
    await waitFor(() => expect(useTurnDetailStore.getState().entries.get("s:0")?.loading).toBe(false));
    for (let turn = 1; turn < 30; turn++) {
      const release = retainTurn("s", turn);
      refreshTurn("s", turn, 1);
      await Promise.resolve();
      release();
    }
    const entries = useTurnDetailStore.getState().entries;
    expect(entries.size).toBe(24);
    expect(entries.has("s:0")).toBe(true);
    expect(entries.has("s:1")).toBe(false);
    keep();
  });

  it("retains partial progress, deduplicates retries and keeps empty delta identities", () => {
    const first = applyStreamRecord({ revision: -1, loading: true }, header);
    const batch: TurnRecord = { kind: "batch", items: [item], operations: [], cursor };
    const partial = applyStreamRecord(first, batch);
    expect(partial.cursor?.since).toBe(-1);
    const duplicate = applyStreamRecord(partial, batch);
    expect(duplicate.turn).toBe(partial.turn);
    const empty = applyStreamRecord(partial, { ...batch, items: [] });
    expect(empty.turn).toBe(partial.turn);
    const reset = applyStreamRecord(partial, { ...header, cursor: { ...cursor, generation: "two" } });
    expect(reset.turn?.items).toEqual([]);
  });

  it("does not replace a newer loaded body with older metadata; updates child totals", () => {
    const pair = { id: "p", name: "bash", is_error: false, is_pending: false, file_touches: [],
      body_version: 10, body_loaded: true, input: { command: "complete" }, result: { content: "output", is_error: false } };
    expect(mergeOperations([pair], [{ ...pair, body_version: 9 }])[0]).toBe(pair);
    const old = [pair];
    expect(mergeOperations(old, [{ ...pair, body_loaded: false, input: {}, result: null }])).toBe(old);
    const updated = mergeOperations(old, [{ ...pair, body_loaded: false, subagent: { title: "child", event_count: 4, turn_count: 1 } }]);
    expect(updated[0]?.input).toBe(pair.input);
    expect(updated[0]?.subagent?.event_count).toBe(4);
  });

  it("coalesces revisions without aborting pending work and reuses completed cache", async () => {
    let finish!: () => void;
    let receive!: (record: TurnRecord) => void;
    const signals: AbortSignal[] = [];
    vi.spyOn(turnStream, "readTurnStream").mockImplementation(async (_session, _turn, _cursor, signal, callback) => {
      signals.push(signal);
      receive = callback;
      if (signals.length === 1) {
        callback(header);
        await new Promise<void>((resolve) => { finish = resolve; });
      }
      callback({ kind: "complete", cursor: { ...cursor, since: 10, through: null } });
    });
    const release = retainTurn("s", 1);
    refreshTurn("s", 1, 1);
    refreshTurn("s", 1, 2);
    refreshTurn("s", 1, 3);
    expect(signals).toHaveLength(1);
    expect(signals[0]?.aborted).toBe(false);
    receive({ kind: "batch", items: [item], operations: [], cursor });
    finish();
    await waitFor(() => expect(useTurnDetailStore.getState().entries.get("s:1")?.revision).toBe(3));
    expect(signals).toHaveLength(2);
    release();
    const releaseAgain = retainTurn("s", 1);
    refreshTurn("s", 1, 3);
    expect(signals).toHaveLength(2);
    releaseAgain();
  });
});
