// A selected turn's detail is read whole once, then kept current by asking
// for what changed after the transcript offset it reflects. Items are
// written once, so a delta's items append; its tool pairs replace the ones
// with the same id.

import type { TimelineChunk, TimelineItem, TimelineTurnDetailResponse } from "../../api/types";
import type { Maybe } from "../../lib/types";
import type { Turn } from "./grouping";

export interface TurnDetailEntry {
  turn: Turn;
  /** Transcript offset the entry reflects, sent back as `since`. */
  through: number;
}

/** Apply a detail response. A whole read replaces the entry; a `since` read
 * merges into the entry it was asked against. Returns null when `entry` is
 * not that base, so the caller reads the turn whole instead. A merged entry
 * keeps the digest of its last whole read: a delta carries none. */
export function applyTurnDetail(
  entry: Maybe<TurnDetailEntry>,
  resp: TimelineTurnDetailResponse,
): TurnDetailEntry | null {
  const turn: Turn = { ...resp.turn, archived_at: resp.archived_at ?? null };
  if (resp.since == null) return { turn, through: resp.through };
  if (!entry || entry.through !== resp.since) return null;

  const seen = new Set(entry.turn.items.map((item) => item.offset));
  const pairs = new Map(entry.turn.tool_pairs.map((pair) => [pair.id, pair] as const));
  for (const pair of resp.turn.tool_pairs) pairs.set(pair.id, pair);
  return {
    turn: {
      ...turn,
      markdown: entry.turn.markdown,
      items: [...entry.turn.items, ...resp.turn.items.filter((item) => !seen.has(item.offset))],
      tool_pairs: [...pairs.values()],
    },
    through: resp.through,
  };
}

/** A rendered block of a turn: an item, or the row of one tool call. */
export type TurnBlock = TimelineChunk | { kind: "tool"; pair_id: string };

/** Render blocks: consecutive assistant items join into one block, which
 * closes at an event that made calls; that event's calls follow it as rows.
 * A block with neither text nor thinking is left out. */
export function groupItems(items: TimelineItem[]): TurnBlock[] {
  const blocks: TurnBlock[] = [];
  let open: Extract<TimelineChunk, { kind: "assistant" }> | null = null;
  const close = () => {
    if (open && (open.items.some((entry) => entry.kind === "text") || open.thinking.length > 0)) {
      blocks.push(open);
    }
    open = null;
  };
  for (const item of items) {
    if (item.kind !== "assistant") {
      close();
      const { offset: _offset, ...chunk } = item;
      blocks.push(chunk as TimelineChunk);
      continue;
    }
    open ??= { kind: "assistant", items: [], thinking: [] };
    open.items.push(...item.items);
    open.thinking.push(...item.thinking);
    const calls = item.items.flatMap((entry) => (entry.kind === "tool" ? [entry.pair_id] : []));
    if (calls.length > 0) {
      close();
      for (const pairId of calls) blocks.push({ kind: "tool", pair_id: pairId });
    }
  }
  close();
  return blocks;
}
