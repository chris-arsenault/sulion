import { authFetch, ApiError } from "./client";
import type { TimelineItem, TimelineSummaryResponse, TimelineToolPair, TimelineTurn } from "./types";
import type { Maybe } from "../lib/types";

export interface TurnCursor {
  turn_id: number;
  generation: string;
  since: number;
  through: number | null;
  item_after: number;
  operation_after: number;
  items_done: boolean;
}

export type TurnRecord =
  | { kind: "header"; turn: TimelineTurn; archived_at: string | null; cursor: TurnCursor; reset: boolean }
  | { kind: "batch"; items: TimelineItem[]; operations: TimelineToolPair[]; cursor: TurnCursor }
  | { kind: "complete"; cursor: TurnCursor }
  | { kind: "reset" }
  | { kind: "error"; message: string };

function turnUrl(session: string, turn: number): string {
  return `/api/timeline/${encodeURIComponent(session)}/turns/${turn}`;
}

export async function readTurnStream(
  session: string, turn: number, cursor: Maybe<TurnCursor>, signal: AbortSignal,
  receive: (record: TurnRecord) => void,
): Promise<void> {
  const params = cursor ? `?${new URLSearchParams({ cursor: JSON.stringify(cursor) })}` : "";
  const response = await authFetch(`${turnUrl(session, turn)}/stream${params}`, { signal });
  if (!response.ok) throw new ApiError(response.status, `Turn request failed (${response.status})`);
  await consumeTurnStream(response, signal, receive);
}

/** Decode records, not network packets: UTF-8 and lines can span any number of reads. */
export async function consumeTurnStream(response: Response, signal: AbortSignal,
  receive: (record: TurnRecord) => void): Promise<void> {
  if (!response.body) throw new Error("Turn response has no body");
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let pending = "";
  let finished = false;
  let yieldedAt = performance.now();
  try {
    while (!finished) {
      signal.throwIfAborted();
      const { value, done } = await reader.read();
      pending += decoder.decode(value, { stream: !done });
      let newline: number;
      while ((newline = pending.indexOf("\n")) >= 0) {
        const line = pending.slice(0, newline);
        pending = pending.slice(newline + 1);
        if (!line.trim()) continue;
        signal.throwIfAborted();
        const record = JSON.parse(line) as TurnRecord;
        if (record.kind === "error") throw new Error(record.message);
        receive(record);
        finished = record.kind === "complete" || record.kind === "reset";
        if (finished) break;
        // Yield after a small processing budget; fast pages do not each incur
        // a full frame of artificial latency.
        if (record.kind === "batch" && performance.now() - yieldedAt >= 8) {
          await new Promise((resolve) => setTimeout(resolve, 0));
          yieldedAt = performance.now();
        }
      }
      if (done && !finished) throw new Error("Turn transfer interrupted; retry to resume");
    }
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}

export async function getTurnDigest(session: string, turn: number): Promise<string> {
  const response = await authFetch(`${turnUrl(session, turn)}/digest`);
  if (!response.ok) throw new ApiError(response.status, "Turn digest request failed");
  return ((await response.json()) as { markdown: string }).markdown;
}

export interface OperationBody {
  id: string;
  generation: string;
  body_version: number;
  input: unknown;
  result: TimelineToolPair["result"];
}

export async function getOperationBodies(session: string, turn: number, ids: string[],
  signal: AbortSignal): Promise<OperationBody[]> {
  const params = new URLSearchParams({ ids: JSON.stringify(ids) });
  const response = await authFetch(`${turnUrl(session, turn)}/operations?${params}`, { signal });
  if (!response.ok) throw new ApiError(response.status, "Tool detail request failed");
  return response.json() as Promise<OperationBody[]>;
}

export async function getTranscriptSummaries(session: string, signal: AbortSignal): Promise<TimelineSummaryResponse> {
  const response = await authFetch(`/api/timeline/${encodeURIComponent(session)}/summaries`, { signal });
  if (!response.ok) throw new ApiError(response.status, "Subagent request failed");
  return response.json() as Promise<TimelineSummaryResponse>;
}
