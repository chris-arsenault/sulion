import type { TimelineItem, TimelineTurn } from "../../api/types";
import type { TimelineFilters } from "./filters";

const filteredAssistant = new WeakMap<TimelineItem, { signature: string; item: TimelineItem }>();

export function filterTurn(turn: TimelineTurn, filters: TimelineFilters): TimelineTurn {
  const pairs = new Map(turn.tool_pairs.map((pair) => [pair.id, pair]));
  const items = turn.items.flatMap((item): TimelineItem[] => {
    if (item.kind === "assistant") {
      if (filters.hiddenSpeakers.has("assistant")) return [];
      const visible = item.items.filter((part) => {
        if (part.kind !== "tool") return true;
        const pair = pairs.get(part.pair_id);
        return !!pair && (!pair.category || !filters.hiddenOperationCategories.has(pair.category));
      });
      if (!visible.length && !item.thinking.length) return [];
      if (visible.length === item.items.length) return [item];
      const signature = visible.filter((part) => part.kind === "tool").map((part) => part.pair_id).join("\0");
      const cached = filteredAssistant.get(item);
      if (cached?.signature === signature) return [cached.item];
      const filtered = { ...item, items: visible };
      filteredAssistant.set(item, { signature, item: filtered });
      return [filtered];
    }
    if (!filters.showBookkeeping && (item.kind === "system" || item.kind === "generic") && item.is_meta) return [];
    return [item];
  });
  const hideResults = filters.hiddenSpeakers.has("tool_result");
  return { ...turn, items,
    tool_pairs: hideResults ? turn.tool_pairs.map((pair) => pair.result ? { ...pair, result: null } : pair) : turn.tool_pairs };
}
