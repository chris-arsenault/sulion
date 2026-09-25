import type { TimelineChunk, TimelineItem } from "../../api/types";

/** A rendered block of a turn: an item, or the row of one tool call. */
export type TurnBlock = TimelineChunk | { kind: "tool"; pair_id: string };

const blockKeys = new WeakMap<TurnBlock, string>();
const blockOffsets = new WeakMap<TurnBlock, number>();
const MAX_ASSISTANT_ITEMS = 64;
export function blockKey(index: number, block: TurnBlock): string {
  return block.kind === "tool" ? `tool:${block.pair_id}` : blockKeys.get(block) ?? `block:${index}`;
}

/** Retain closed blocks when immutable items append. Only the trailing,
 * unfinished assistant block can join the next batch. */
export function createItemGrouper() {
  let previous: TimelineItem[] = [];
  let blocks: TurnBlock[] = [];
  let tailStart = 0;
  let tailBlock = 0;
  return (items: TimelineItem[]): TurnBlock[] => {
    if (items === previous) return blocks;
    const append = items.length >= previous.length && previous.every((item, i) => items[i] === item);
    if (append && items.length === previous.length) return blocks;
    blocks = append ? [...blocks.slice(0, tailBlock), ...groupItems(items.slice(tailStart))] : groupItems(items);
    tailStart = items.length;
    const last = blocks.at(-1);
    if (last?.kind === "assistant") {
      const offset = blockOffsets.get(last)!;
      while (tailStart > 0 && items[tailStart - 1]!.offset >= offset) tailStart -= 1;
      if (items.length - tailStart >= MAX_ASSISTANT_ITEMS) tailStart = items.length;
    }
    tailBlock = blocks.length - (tailStart < items.length && blocks.at(-1)?.kind === "assistant" ? 1 : 0);
    previous = items;
    return blocks;
  };
}

/** Render blocks: consecutive assistant items join into a bounded block, which
 * also closes at an event that made calls; that event's calls follow it as rows.
 * A block with neither text nor thinking is left out. */
export function groupItems(items: TimelineItem[]): TurnBlock[] {
  const blocks: TurnBlock[] = [];
  let open: Extract<TimelineChunk, { kind: "assistant" }> | null = null;
  let openOffset = 0;
  let openCount = 0;
  const close = () => {
    if (open && (open.items.some((entry) => entry.kind === "text") || open.thinking.length > 0)) {
      blockKeys.set(open, `item:${openOffset}`);
      blockOffsets.set(open, openOffset);
      blocks.push(open);
    }
    open = null;
    openCount = 0;
  };
  for (const item of items) {
    if (item.kind !== "assistant") {
      close();
      const { offset: _offset, ...chunk } = item;
      blockKeys.set(chunk as TurnBlock, `item:${_offset}`);
      blocks.push(chunk as TimelineChunk);
      continue;
    }
    if (!open) {
      open = { kind: "assistant", items: [], thinking: [] };
      openOffset = item.offset;
    }
    open.items.push(...item.items);
    open.thinking.push(...item.thinking);
    openCount += 1;
    const calls = item.items.flatMap((entry) => (entry.kind === "tool" ? [entry.pair_id] : []));
    if (calls.length > 0) {
      close();
      for (const pairId of calls) blocks.push({ kind: "tool", pair_id: pairId });
    } else if (openCount >= MAX_ASSISTANT_ITEMS) {
      close();
    }
  }
  close();
  return blocks;
}
