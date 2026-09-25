import { create } from "zustand";
import { readTurnStream, type TurnCursor, type TurnRecord } from "../api/turnStream";
import { requestBody } from "./turnBodies";
import type { TimelineTurn, TimelineToolPair } from "../api/types";

export interface StreamEntry {
  turn?: TimelineTurn;
  cursor?: TurnCursor;
  revision: number;
  loading: boolean;
  error?: string;
}

interface DetailState { entries: Map<string, StreamEntry> }
export const useTurnDetailStore = create<DetailState>(() => ({ entries: new Map() }));
const MAX_TURNS = 24;
const readers = new Map<string, { users: number; desired: number; controller?: AbortController }>();
export const detailKey = (session: string, turn: number) => `${session}:${turn}`;

function put(key: string, entry: StreamEntry) {
  useTurnDetailStore.setState((state) => {
    const entries = new Map(state.entries);
    entries.delete(key);
    entries.set(key, entry);
    for (const candidate of entries.keys()) {
      if (entries.size <= MAX_TURNS) break;
      if (!readers.has(candidate)) entries.delete(candidate);
    }
    return { entries };
  });
}

export function mergeOperations(old: TimelineToolPair[], updates: TimelineToolPair[]): TimelineToolPair[] {
  if (!updates.length) return old;
  const byId = new Map(old.map((pair) => [pair.id, pair]));
  let changed = false;
  for (const update of updates) {
    const prior = byId.get(update.id);
    if (prior && (prior.body_version ?? -1) > (update.body_version ?? -1)) continue;
    const pair = prior?.body_loaded && prior.body_version === update.body_version
      ? { ...update, input: prior.input, result: prior.result, body_loaded: true } : update;
    if (prior && prior.body_version === pair.body_version &&
      prior.is_error === pair.is_error && prior.is_pending === pair.is_pending &&
      prior.name === pair.name && prior.raw_name === pair.raw_name &&
      prior.operation_type === pair.operation_type && prior.category === pair.category &&
      JSON.stringify(prior.file_touches) === JSON.stringify(pair.file_touches) &&
      JSON.stringify(prior.subagent) === JSON.stringify(pair.subagent)) continue;
    byId.set(pair.id, pair);
    changed = true;
  }
  return changed ? [...byId.values()] : old;
}

export function applyStreamRecord(entry: StreamEntry, record: TurnRecord): StreamEntry {
  if (record.kind === "reset") return { revision: -1, loading: true };
  if (record.kind === "error") return { ...entry, error: record.message, loading: false };
  if (record.kind === "complete") return { ...entry, cursor: record.cursor };
  if (record.kind === "header") {
    const prior = record.reset ? undefined : entry.turn;
    const turn = {
      ...record.turn, archived_at: record.archived_at, generation: record.cursor.generation,
      items: prior?.items ?? [], tool_pairs: prior?.tool_pairs ?? [],
    };
    const unchanged = prior && (Object.keys(turn) as Array<keyof TimelineTurn>)
      .every((key) => turn[key] === prior[key]);
    return { ...entry, cursor: record.cursor, turn: unchanged ? prior : turn };
  }
  if (!entry.turn) throw new Error("Turn batch arrived before its header");
  const old = entry.turn;
  const last = old.items.at(-1)?.offset ?? -1;
  const appended = record.items.filter((item) => item.offset > last);
  const items = appended.length ? [...old.items, ...appended] : old.items;
  const pairs = mergeOperations(old.tool_pairs, record.operations);
  return { ...entry, cursor: record.cursor,
    turn: items === old.items && pairs === old.tool_pairs ? old : { ...old, items, tool_pairs: pairs } };
}

export function retainTurn(session: string, turn: number): () => void {
  const key = detailKey(session, turn);
  const reader = readers.get(key) ?? { users: 0, desired: -1 };
  reader.users += 1;
  readers.set(key, reader);
  return () => {
    reader.users -= 1;
    if (reader.users === 0) {
      reader.controller?.abort();
      readers.delete(key);
      const entry = useTurnDetailStore.getState().entries.get(key);
      if (entry) put(key, { ...entry, loading: false });
    }
  };
}

export function refreshTurn(session: string, turn: number, revision: number, retry = false) {
  const key = detailKey(session, turn);
  const reader = readers.get(key);
  if (!reader) return;
  reader.desired = revision;
  if (reader.controller) return;
  const entry = useTurnDetailStore.getState().entries.get(key);
  if (!retry && entry?.revision === revision && entry.cursor?.through === null) return;
  const controller = new AbortController();
  reader.controller = controller;
  void (async () => {
    let resets = 0;
    try {
      while (!controller.signal.aborted) {
        const requested = reader.desired;
        let current = useTurnDetailStore.getState().entries.get(key) ?? { revision: -1, loading: true };
        current = { ...current, loading: true, error: undefined };
        put(key, current);
        let reset = false;
        await readTurnStream(session, turn, current.cursor, controller.signal, (record) => {
          current = applyStreamRecord(useTurnDetailStore.getState().entries.get(key) ?? current, record);
          reset ||= record.kind === "reset";
          put(key, current);
        });
        if (reset) {
          if (++resets > 2) throw new Error("Timeline is rebuilding; retry shortly");
          continue;
        }
        put(key, { ...current, revision: requested, loading: reader.desired !== requested });
        if (reader.desired === requested) break;
      }
    } catch (error) {
      if (!controller.signal.aborted) {
        const current = useTurnDetailStore.getState().entries.get(key);
        put(key, { ...current, revision: current?.revision ?? -1, loading: false,
          error: error instanceof Error ? error.message : "Turn loading failed" });
      }
    } finally {
      if (reader.controller === controller) reader.controller = undefined;
    }
  })();
}

export async function hydrateOperation(session: string, turn: number, pair: string, signal: AbortSignal) {
  const body = await requestBody(session, turn, pair, signal);
  if (signal.aborted) return;
  const key = detailKey(session, turn);
  const entry = useTurnDetailStore.getState().entries.get(key);
  if (!entry?.turn || entry.turn.generation !== body.generation) return;
  const pairs = entry.turn.tool_pairs.map((value) => value.id === pair &&
    (value.body_version ?? -1) <= body.body_version
    ? { ...value, input: body.input, result: body.result, body_version: body.body_version, body_loaded: true } : value);
  put(key, { ...entry, turn: { ...entry.turn, tool_pairs: pairs } });
}

export function resetTurnDetails() {
  for (const reader of readers.values()) reader.controller?.abort();
  readers.clear();
  useTurnDetailStore.setState({ entries: new Map() });
}
