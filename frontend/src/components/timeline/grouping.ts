import type { TimelineEvent } from "../../api/types";
import {
  isAssistantEvent,
  isBookkeepingEvent,
  isRealUserPrompt,
  isSidechainEvent,
  isToolResultEvent,
  thinkingBlocksIn,
  toolResultsIn,
  toolUsesIn,
  type ToolResultBlock,
  type ToolUseBlock,
} from "./types";

// Claude Code writes many thinking blocks with an empty `thinking`
// field (signature-only). They round-trip through the transcript but
// carry nothing for the UI to render, so we exclude them everywhere —
// count, chips, export — to keep the UI honest.
function hasUsefulThinking(ev: TimelineEvent): boolean {
  return thinkingBlocksIn(ev).some(
    (t) => typeof t.thinking === "string" && t.thinking.trim().length > 0,
  );
}

// A "turn" is a user prompt plus every event that follows it up to the
// next real user prompt. Bookkeeping events (file-history-snapshot,
// permission-mode, last-prompt, queue-operation, attachment, isMeta
// system) are dropped before grouping. Sidechain events are optionally
// dropped depending on the filter (see applyFilters).
export interface Turn {
  /** Stable id for virtuoso keying — byte_offset of the opening event. */
  id: number;
  /** The opening user prompt event, if present. Null means an orphan
   * turn (events before the first real prompt, e.g. resume-session
   * bootstrap). */
  userPrompt: TimelineEvent | null;
  /** All events inside this turn, in arrival order, including the
   * opening user prompt when present. */
  events: TimelineEvent[];
  /** ISO strings for the first and last event's timestamp. */
  startTimestamp: string;
  endTimestamp: string;
  durationMs: number;
  /** Resolved tool-use → tool-result pairs, in the order the tool_use
   * appeared. When the result hasn't arrived (still pending or lost),
   * result is null and isPending is true. */
  toolPairs: ToolPair[];
  /** Count of assistant events containing thinking blocks. */
  thinkingCount: number;
  /** True if any tool result in this turn has is_error. */
  hasErrors: boolean;
}

export interface ToolPair {
  /** tool_use_id — stable across the use/result pair. */
  id: string;
  name: string;
  input: unknown;
  use: ToolUseBlock;
  useEvent: TimelineEvent;
  result: ToolResultBlock | null;
  resultEvent: TimelineEvent | null;
  isError: boolean;
  isPending: boolean;
}

export function groupIntoTurns(events: TimelineEvent[]): Turn[] {
  const turns: Turn[] = [];
  let current: Turn | null = null;

  for (const ev of events) {
    if (isRealUserPrompt(ev)) {
      current = newTurn(ev);
      turns.push(current);
      continue;
    }
    if (current == null) {
      // Orphan: events before any real user prompt. Create a synthetic
      // turn so nothing is silently dropped.
      current = newTurn(null, ev);
      turns.push(current);
    }
    current.events.push(ev);
    current.endTimestamp = ev.timestamp;
    current.durationMs = durationMsBetween(
      current.startTimestamp,
      current.endTimestamp,
    );
  }

  // Second pass: pair tool_use with tool_result, count thinking, detect errors.
  for (const turn of turns) {
    const uses = new Map<
      string,
      { block: ToolUseBlock; event: TimelineEvent; order: number }
    >();
    const results = new Map<
      string,
      { block: ToolResultBlock; event: TimelineEvent }
    >();
    let useOrder = 0;

    for (const ev of turn.events) {
      if (isAssistantEvent(ev)) {
        for (const use of toolUsesIn(ev)) {
          const id = use.id ?? `noid-${useOrder}`;
          uses.set(id, { block: use, event: ev, order: useOrder++ });
        }
        if (hasUsefulThinking(ev)) turn.thinkingCount += 1;
      }
      if (isToolResultEvent(ev)) {
        for (const result of toolResultsIn(ev)) {
          const id = result.tool_use_id ?? `noid-${useOrder}`;
          results.set(id, { block: result, event: ev });
          if (result.is_error === true) turn.hasErrors = true;
        }
      }
    }

    turn.toolPairs = Array.from(uses.values())
      .sort((a, b) => a.order - b.order)
      .map(({ block, event }) => {
        const id = block.id ?? "";
        const match = id ? results.get(id) : undefined;
        return {
          id,
          name: block.name ?? "unknown",
          input: block.input,
          use: block,
          useEvent: event,
          result: match?.block ?? null,
          resultEvent: match?.event ?? null,
          isError: match?.block?.is_error === true,
          isPending: match == null,
        } satisfies ToolPair;
      });
  }

  return turns;
}

function newTurn(
  prompt: TimelineEvent | null,
  seed?: TimelineEvent,
): Turn {
  const first = prompt ?? seed!;
  return {
    id: first.byte_offset,
    userPrompt: prompt,
    events: prompt ? [prompt] : [],
    startTimestamp: first.timestamp,
    endTimestamp: first.timestamp,
    durationMs: 0,
    toolPairs: [],
    thinkingCount: 0,
    hasErrors: false,
  };
}

function durationMsBetween(a: string, b: string): number {
  const da = new Date(a).getTime();
  const db = new Date(b).getTime();
  if (Number.isNaN(da) || Number.isNaN(db)) return 0;
  return Math.max(0, db - da);
}

/** Pre-filter: drop events that should never reach the grouper. Bookkeeping
 * always; sidechain only when showSidechain is false (default). */
export function prefilter(
  events: TimelineEvent[],
  opts: { showBookkeeping: boolean; showSidechain: boolean },
): TimelineEvent[] {
  return events.filter((ev) => {
    if (!opts.showBookkeeping && isBookkeepingEvent(ev)) return false;
    if (!opts.showSidechain && isSidechainEvent(ev)) return false;
    return true;
  });
}
