import type { TimelineBlock, TimelineEvent } from "../../api/types";
import type { Maybe } from "../../lib/types";

// ─── Helpers over the canonical block model ────────────────────────
//
// Historically these helpers probed `event.payload.message.content`
// with shape checks. That tied every renderer to Claude Code's
// specific JSONL layout. With the canonical-blocks migration the
// backend parses each event into a stable `TimelineBlock[]`, and
// these helpers read from there. When we plug in a second agent
// (Codex, etc.) the payload shape is irrelevant — the block list is
// identical.
//
// For any event the ingester hasn't backfilled yet, `event.blocks` is
// empty; we fall back to legacy payload probing so pre-migration rows
// keep rendering. Remove the fallback once the backfill completes.

/** Legacy payload shape, kept only for the fallback path on events
 * that predate canonical-block backfill. New code should read
 * `event.blocks` directly. */
export interface EventPayload {
  type?: string;
  timestamp?: string;
  uuid?: string;
  sessionId?: string;
  parentUuid?: string | null;
  isSidechain?: boolean;
  isMeta?: boolean;
  isCompactSummary?: boolean;
  message?: MessagePayload;
  subtype?: string;
  [key: string]: unknown;
}

export interface MessagePayload {
  role?: "user" | "assistant" | string;
  content?: string | LegacyContentBlock[];
  stop_reason?: string;
  usage?: Record<string, unknown>;
}

type LegacyContentBlock =
  | { type: "text"; text: string }
  | { type: "thinking"; thinking?: string; signature?: string }
  | { type: "tool_use"; id?: string; name?: string; input?: unknown }
  | {
      type: "tool_result";
      tool_use_id?: string;
      content?: string | LegacyContentBlock[];
      is_error?: boolean;
    }
  | { type: string; [key: string]: unknown };

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

export function payloadOf(event: TimelineEvent): EventPayload {
  if (event.payload && typeof event.payload === "object") {
    return event.payload as EventPayload;
  }
  return {};
}

/** Canonical blocks with backward-compat fallback for events predating
 * the canonical-block migration. */
function blocksOf(event: TimelineEvent): TimelineBlock[] {
  if (Array.isArray(event.blocks) && event.blocks.length > 0) {
    return event.blocks;
  }
  return legacyBlocksFromPayload(event);
}

function legacyBlocksFromPayload(event: TimelineEvent): TimelineBlock[] {
  const content = payloadOf(event).message?.content;
  if (typeof content === "string") {
    return [{ ord: 0, kind: "text", text: content }];
  }
  if (!Array.isArray(content)) return [];
  const out: TimelineBlock[] = [];
  let ord = 0;
  for (const b of content) {
    if (b.type === "text") {
      out.push({ ord: ord++, kind: "text", text: (b as { text?: string }).text ?? "" });
    } else if (b.type === "thinking") {
      out.push({
        ord: ord++,
        kind: "thinking",
        text: (b as { thinking?: string }).thinking ?? "",
      });
    } else if (b.type === "tool_use") {
      const name = (b as { name?: string }).name ?? "";
      out.push({
        ord: ord++,
        kind: "tool_use",
        tool_id: (b as { id?: string }).id ?? "",
        tool_name: name,
        tool_name_canonical: name.toLowerCase(),
        tool_input: (b as { input?: unknown }).input ?? null,
      });
    } else if (b.type === "tool_result") {
      const tr = b as {
        tool_use_id?: string;
        content?: string | LegacyContentBlock[];
        is_error?: boolean;
      };
      let text: Maybe<string>;
      if (typeof tr.content === "string") text = tr.content;
      else if (Array.isArray(tr.content)) {
        text = tr.content
          .filter((c): c is { type: "text"; text: string } => c.type === "text")
          .map((c) => c.text)
          .join("\n");
      }
      out.push({
        ord: ord++,
        kind: "tool_result",
        tool_id: tr.tool_use_id ?? "",
        text,
        is_error: tr.is_error ?? false,
      });
    } else {
      out.push({ ord: ord++, kind: "unknown", raw: b });
    }
  }
  return out;
}

export function isBookkeepingEvent(event: TimelineEvent): boolean {
  if (BOOKKEEPING_KINDS.has(event.kind)) return true;
  if (event.kind === "system") {
    const p = payloadOf(event);
    if (p.isMeta === true) return true;
  }
  return false;
}

export function isSidechainEvent(event: TimelineEvent): boolean {
  return payloadOf(event).isSidechain === true;
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
    else if (b.kind === "tool_result")
      parts.push(`[tool_result]${b.text ? ` ${b.text}` : ""}`);
    // thinking intentionally excluded
  }
  return parts.join(" ");
}

/** Legacy shape-based flattener kept for callers that receive an
 * already-extracted content array (summary panels, markdown export for
 * raw tool_result content). Requires non-null content — callers with
 * optional sources pass `content ?? ""` so the absent case is handled
 * at the call site rather than hidden inside the helper. */
export function flattenContent(
  content: string | LegacyContentBlock[],
): string {
  if (typeof content === "string") return content;
  const parts: string[] = [];
  for (const block of content) {
    if (block.type === "text" && typeof block.text === "string") {
      parts.push(block.text);
    } else if (block.type === "tool_use" && block.name) {
      parts.push(`[tool_use: ${block.name}]`);
    } else if (block.type === "tool_result") {
      const tr = block as { content?: string | LegacyContentBlock[] };
      const nested =
        typeof tr.content === "string"
          ? tr.content
          : tr.content
            ? flattenContent(tr.content)
            : "";
      parts.push(`[tool_result]${nested ? ` ${nested}` : ""}`);
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
export function isToolResultUser(event: TimelineEvent): boolean {
  if (event.kind !== "user") return false;
  return blocksOf(event).some((b) => b.kind === "tool_result");
}

/** True when a user event carries an actual typed prompt. */
export function isRealUserPrompt(event: TimelineEvent): boolean {
  if (event.kind !== "user") return false;
  if (isToolResultUser(event)) return false;
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
