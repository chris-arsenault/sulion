import type {
  OperationCategory,
  TimelineAssistantItem,
  TimelineBlock,
  TimelineChunk,
  TimelineEvent,
  TimelineSubagent,
  TimelineToolPair,
  TimelineTurn,
} from "../../api/types";

function speakerForKind(kind: string): string {
  switch (kind) {
    case "user":
      return "user";
    case "assistant":
      return "assistant";
    case "system":
      return "system";
    case "summary":
      return "summary";
    default:
      return "other";
  }
}

function contentKindForBlocks(blocks: TimelineBlock[]): string {
  const kinds = new Set(blocks.map((b) => b.kind));
  if (kinds.size === 0) return "none";
  if (kinds.size > 1) return "mixed";
  return blocks[0]?.kind ?? "none";
}

export function makeEvent(
  kind: string,
  overrides: Partial<TimelineEvent> = {},
): TimelineEvent {
  const blocks = overrides.blocks ?? [];
  return {
    byte_offset: overrides.byte_offset ?? 0,
    timestamp: overrides.timestamp ?? "2025-01-01T00:00:00Z",
    kind,
    agent: overrides.agent ?? "claude-code",
    speaker: overrides.speaker ?? speakerForKind(kind),
    content_kind: overrides.content_kind ?? contentKindForBlocks(blocks),
    event_uuid: overrides.event_uuid ?? null,
    parent_event_uuid: overrides.parent_event_uuid ?? null,
    related_tool_use_id: overrides.related_tool_use_id ?? null,
    is_sidechain: overrides.is_sidechain ?? false,
    is_meta: overrides.is_meta ?? false,
    subtype: overrides.subtype ?? null,
    blocks,
  };
}

export function textBlock(ord: number, text: string): TimelineBlock {
  return { ord, kind: "text", text };
}

export function toolUseBlock(
  ord: number,
  id: string,
  canonicalName: string,
  input: unknown,
  rawName?: string,
  operationCategory?: OperationCategory,
): TimelineBlock {
  return {
    ord,
    kind: "tool_use",
    tool_id: id,
    tool_name: rawName ?? canonicalName,
    tool_name_canonical: canonicalName,
    operation_category: operationCategory,
    tool_input: input,
  };
}

export function toolResultBlock(
  ord: number,
  toolUseId: string,
  text?: string,
  isError = false,
): TimelineBlock {
  return {
    ord,
    kind: "tool_result",
    tool_id: toolUseId,
    text,
    is_error: isError,
  };
}

export function makePair(
  overrides: Partial<TimelineToolPair> = {},
): TimelineToolPair {
  return {
    id: overrides.id ?? "t1",
    name: overrides.name ?? "bash",
    raw_name: overrides.raw_name ?? null,
    category: overrides.category ?? "utility",
    input: overrides.input ?? {},
    result: overrides.result ?? null,
    is_error: overrides.is_error ?? false,
    is_pending: overrides.is_pending ?? false,
    file_touches: overrides.file_touches ?? [],
    subagent: overrides.subagent ?? null,
  };
}

export function assistantItems(
  ...items: Array<string | { tool: string }>
): TimelineAssistantItem[] {
  return items.map((item) =>
    typeof item === "string"
      ? { kind: "text", text: item }
      : { kind: "tool", pair_id: item.tool },
  );
}

export function assistantChunk(
  items: TimelineAssistantItem[],
  thinking: string[] = [],
): TimelineChunk {
  return { kind: "assistant", items, thinking };
}

export function toolChunk(pairId: string): TimelineChunk {
  return { kind: "tool", pair_id: pairId };
}

export function makeTurn(
  overrides: Partial<TimelineTurn> = {},
): TimelineTurn {
  return {
    id: overrides.id ?? 1,
    preview: overrides.preview ?? "prompt",
    user_prompt_text: overrides.user_prompt_text ?? "prompt",
    start_timestamp: overrides.start_timestamp ?? "2025-01-01T00:00:00Z",
    end_timestamp: overrides.end_timestamp ?? "2025-01-01T00:00:02Z",
    duration_ms: overrides.duration_ms ?? 2000,
    event_count: overrides.event_count ?? 2,
    operation_count: overrides.operation_count ?? (overrides.tool_pairs?.length ?? 0),
    tool_pairs: overrides.tool_pairs ?? [],
    thinking_count: overrides.thinking_count ?? 0,
    has_errors: overrides.has_errors ?? false,
    input_tokens: overrides.input_tokens ?? 0,
    output_tokens: overrides.output_tokens ?? 0,
    markdown: overrides.markdown ?? "**Prompt**\n\n> prompt",
    chunks: overrides.chunks ?? [],
  };
}

export function makeSubagent(
  overrides: Partial<TimelineSubagent> = {},
): TimelineSubagent {
  return {
    title: overrides.title ?? "Agent log",
    event_count: overrides.event_count ?? 0,
    turns: overrides.turns ?? [],
  };
}
