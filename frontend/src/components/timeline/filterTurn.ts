import type { TimelineItem, TimelineTurn } from "../../api/types";
import type { TimelineFilters } from "./filters";

const bookkeeping = new Set([
  "agent-name", "ai-title", "attachment", "bridge-session", "custom-title",
  "file-history-delta", "file-history-snapshot", "frame-link", "last-prompt",
  "mode", "permission-mode", "queue-operation", "world_state", "token_usage_record", "atis-latch",
]);
const telemetry = new Set(["stop_hook_summary", "turn_duration", "runtime_evidence"]);
const commandPrefix = /^\s*<(command-name|command-message|local-command-stdout|local-command-stderr|local-command-caveat)>/;
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
    if (!filters.showBookkeeping) {
      if (item.kind === "system" && (item.is_meta || telemetry.has(item.subtype ?? ""))) return [];
      if (item.kind === "generic" && (bookkeeping.has(item.label) || commandPrefix.test(
        item.details.blocks.find((block) => block.kind === "text")?.text ?? "",
      ))) return [];
    }
    return [item];
  });
  const hideResults = filters.hiddenSpeakers.has("tool_result");
  return { ...turn, items,
    tool_pairs: hideResults ? turn.tool_pairs.map((pair) => pair.result ? { ...pair, result: null } : pair) : turn.tool_pairs };
}
