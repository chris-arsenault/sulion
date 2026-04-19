import type { TimelineBlock, TimelineEvent } from "../../api/types";

/** Shape matched by callers that group tool_use → tool_result pairs. */
export interface ToolUseBlock {
  type: "tool_use";
  id?: string;
  /** Canonical tool name (e.g. "read", "bash"). Renderers switch on this. */
  name?: string;
  /** Raw name as emitted by the agent. Available for display / debug. */
  rawName?: string;
  input?: unknown;
}

export interface ToolResultBlock {
  type: "tool_result";
  tool_use_id?: string;
  content?: string;
  is_error?: boolean;
}

export interface ThinkingBlock {
  type: "thinking";
  thinking?: string;
}

// Top-level event kinds surfaced by Claude Code's JSONL that carry
// purely internal / filesystem / housekeeping state, not conversational
// content. Hidden from the default timeline view.
const BOOKKEEPING_KINDS = new Set<string>([
  "file-history-snapshot",
  "permission-mode",
  "last-prompt",
  "queue-operation",
  "attachment",
]);

function blocksOf(event: TimelineEvent): TimelineBlock[] {
  return Array.isArray(event.blocks) ? event.blocks : [];
}

export function eventSpeaker(event: TimelineEvent): string {
  if (event.speaker) return event.speaker;
  if (event.kind === "assistant") return "assistant";
  if (event.kind === "user") return "user";
  if (event.kind === "system") return "system";
  if (event.kind === "summary") return "summary";
  return "other";
}

export function isAssistantEvent(event: TimelineEvent): boolean {
  return eventSpeaker(event) === "assistant";
}

export function isSystemEvent(event: TimelineEvent): boolean {
  return eventSpeaker(event) === "system";
}

export function isSummaryEvent(event: TimelineEvent): boolean {
  return eventSpeaker(event) === "summary";
}

export function isBookkeepingEvent(event: TimelineEvent): boolean {
  if (BOOKKEEPING_KINDS.has(event.kind)) return true;
  return isSystemEvent(event) && event.is_meta === true;
}

export function isSidechainEvent(event: TimelineEvent): boolean {
  return event.is_sidechain === true;
}

/** A flattened chunk of text suitable for a preview line or block header.
 * Excludes `thinking` and tool_use input — those get their own renderers. */
export function textPreview(event: TimelineEvent, max = 140): string {
  const text = flattenEventContent(event);
  if (text) return trim(text, max);
  return trim(`[${event.kind}]`, max);
}

/** Flatten an event's blocks into a single short string for previews. */
export function flattenEventContent(event: TimelineEvent): string {
  const parts: string[] = [];
  for (const b of blocksOf(event)) {
    if (b.kind === "text" && b.text) parts.push(b.text);
    else if (b.kind === "tool_use") parts.push(`[tool_use: ${b.tool_name_canonical ?? ""}]`);
    else if (b.kind === "tool_result") {
      parts.push(`[tool_result]${b.text ? ` ${b.text}` : ""}`);
    }
  }
  return parts.join(" ");
}

function trim(s: string, max: number): string {
  const oneLine = s.replace(/\s+/g, " ").trim();
  if (oneLine.length <= max) return oneLine;
  return `${oneLine.slice(0, max - 1)}…`;
}

/** True when a user event is actually a tool_result container and should
 * fold into the preceding assistant turn rather than open a new turn. */
export function isToolResultEvent(event: TimelineEvent): boolean {
  return blocksOf(event).some((b) => b.kind === "tool_result");
}

export function isToolResultUser(event: TimelineEvent): boolean {
  return eventSpeaker(event) === "user" && isToolResultEvent(event);
}

/** True when a user event carries an actual typed prompt. */
export function isRealUserPrompt(event: TimelineEvent): boolean {
  if (eventSpeaker(event) !== "user") return false;
  if (isToolResultEvent(event)) return false;
  return true;
}

export function toolUsesIn(event: TimelineEvent): ToolUseBlock[] {
  return blocksOf(event)
    .filter((b) => b.kind === "tool_use")
    .map((b) => ({
      type: "tool_use",
      id: b.tool_id,
      name: b.tool_name_canonical ?? b.tool_name,
      rawName: b.tool_name,
      input: b.tool_input,
    }));
}

export function toolResultsIn(event: TimelineEvent): ToolResultBlock[] {
  return blocksOf(event)
    .filter((b) => b.kind === "tool_result")
    .map((b) => ({
      type: "tool_result",
      tool_use_id: b.tool_id,
      content: b.text,
      is_error: b.is_error,
    }));
}

export function thinkingBlocksIn(event: TimelineEvent): ThinkingBlock[] {
  return blocksOf(event)
    .filter((b) => b.kind === "thinking")
    .map((b) => ({ type: "thinking", thinking: b.text ?? "" }));
}

export function hasThinking(event: TimelineEvent): boolean {
  return thinkingBlocksIn(event).length > 0;
}

export function textBlocksIn(event: TimelineEvent): string[] {
  return blocksOf(event)
    .filter((b) => b.kind === "text")
    .map((b) => b.text ?? "");
}

/** Text of a user event's real prompt content, if any. */
export function userPromptText(event: TimelineEvent): string {
  if (!isRealUserPrompt(event)) return "";
  return textBlocksIn(event).join(" ");
}

/** True when any tool_result block in this event has is_error=true. */
export function hasToolError(event: TimelineEvent): boolean {
  return toolResultsIn(event).some((b) => b.is_error === true);
}
