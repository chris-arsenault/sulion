import type { TimelineBlock, TimelineEvent } from "../../api/types";

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

export function thinkingBlock(ord: number, text: string): TimelineBlock {
  return { ord, kind: "thinking", text };
}

export function toolUseBlock(
  ord: number,
  id: string,
  canonicalName: string,
  input: unknown,
  rawName?: string,
): TimelineBlock {
  return {
    ord,
    kind: "tool_use",
    tool_id: id,
    tool_name: rawName ?? canonicalName,
    tool_name_canonical: canonicalName,
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
