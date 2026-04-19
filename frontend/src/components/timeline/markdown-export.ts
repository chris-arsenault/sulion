// Markdown formatters for copy-as-markdown (ticket #20). Converts a
// turn or individual assistant event into a portable markdown document
// suitable for pasting into GitHub, Slack, a PR description, etc.

import type { TimelineEvent } from "../../api/types";
import type { ToolPair, Turn } from "./grouping";
import {
  isAssistantEvent,
  isToolResultEvent,
  textBlocksIn,
  userPromptText,
} from "./types";

/** Full turn as markdown — prompt + all assistant events + tool calls
 * in arrival order. Used by the "Copy turn" button. */
export function formatTurn(turn: Turn): string {
  const parts: string[] = [];

  if (turn.userPrompt) {
    const prompt = userPromptText(turn.userPrompt);
    if (prompt.trim().length > 0) {
      parts.push(formatPrompt(prompt));
    }
  }

  const pairById = new Map(turn.toolPairs.map((p) => [p.id, p] as const));

  for (const ev of turn.events) {
    if (ev === turn.userPrompt) continue;
    if (isToolResultEvent(ev)) continue; // surfaced via pairs
    if (isAssistantEvent(ev)) {
      parts.push(formatAssistantEvent(ev, pairById));
    }
    // Skip system/summary/unknown — they're noise in the markdown export.
  }

  return parts.join("\n\n");
}

/** Just the assistant prose from one event (no tool calls). Used by
 * the per-event "Copy response" button. */
export function formatAssistantText(event: TimelineEvent): string {
  const texts = textBlocksIn(event);
  return texts.join("\n\n").trim();
}

/** One assistant event rendered as markdown — its text blocks plus
 * each tool_use rendered inline. */
export function formatAssistantEvent(
  event: TimelineEvent,
  pairById: Map<string, ToolPair>,
): string {
  const parts: string[] = [];
  for (const block of event.blocks) {
    if (block.kind === "text") {
      const t = (block.text ?? "").trim();
      if (t) parts.push(t);
      continue;
    }
    if (block.kind === "tool_use") {
      const id = block.tool_id;
      const pair = id ? pairById.get(id) : undefined;
      if (pair) parts.push(formatToolPair(pair));
      continue;
    }
    // thinking blocks are intentionally omitted from copy output:
    // they're empty in almost all transcripts (signature-only).
  }
  return parts.join("\n\n");
}

/** A tool pair: tool-name + input, then result (truncated) if present. */
export function formatToolPair(pair: ToolPair): string {
  const header = `**Tool:** \`${pair.name}\`${toolOneLine(pair)}`;
  const inputBlock = formatToolInput(pair);
  const resultBlock = formatToolResult(pair);
  const status = pair.isPending ? " _(pending)_" : pair.isError ? " _(error)_" : "";
  return [`${header}${status}`, inputBlock, resultBlock].filter(Boolean).join("\n\n");
}

function formatPrompt(text: string): string {
  // Markdown blockquote per paragraph so multi-paragraph prompts
  // quote cleanly.
  const quoted = text
    .split(/\n/)
    .map((line) => `> ${line}`)
    .join("\n");
  return `**Prompt**\n\n${quoted}`;
}

function toolOneLine(pair: ToolPair): string {
  const input = (pair.input ?? {}) as Record<string, unknown>;
  const pick = (k: string) =>
    typeof input[k] === "string" ? (input[k] as string) : undefined;
  let summary = "";
  switch (pair.name) {
    case "edit":
    case "write":
    case "multi_edit":
    case "read":
      summary = pick("path") ?? "";
      break;
    case "bash":
      summary = pick("command") ?? "";
      break;
    case "grep":
    case "glob":
      summary = pick("pattern") ?? "";
      break;
    case "task":
      summary = pick("description") ?? pick("agent") ?? "";
      break;
    case "web_fetch":
      summary = pick("url") ?? "";
      break;
    case "web_search":
      summary = pick("query") ?? "";
      break;
  }
  return summary ? ` \`${summary.slice(0, 160)}\`` : "";
}

function formatToolInput(pair: ToolPair): string {
  const input = pair.input;
  if (pair.name === "edit" || pair.name === "write") {
    return formatEditInput(pair);
  }
  if (pair.name === "multi_edit") {
    return formatMultiEditInput(pair);
  }
  if (pair.name === "bash") {
    const cmd =
      typeof (input as { command?: unknown })?.command === "string"
        ? ((input as { command: string }).command)
        : "";
    if (!cmd) return "";
    return fence("bash", cmd);
  }
  if (pair.name === "todo_write") {
    const todos = (input as { todos?: Array<{ status?: string; content?: string }> })
      ?.todos;
    if (!Array.isArray(todos) || todos.length === 0) return "";
    const lines = todos.map((t) => {
      const box =
        t.status === "completed" ? "[x]" : t.status === "in_progress" ? "[~]" : "[ ]";
      return `- ${box} ${t.content ?? ""}`;
    });
    return lines.join("\n");
  }
  // Generic: pretty-print JSON
  return fence("json", JSON.stringify(input ?? {}, null, 2));
}

function formatEditInput(pair: ToolPair): string {
  const input = pair.input as Record<string, unknown>;
  const oldStr = typeof input.old_text === "string" ? input.old_text : "";
  const newStr = typeof input.new_text === "string" ? input.new_text : "";
  if (!oldStr && !newStr) return "";
  const diff = unifiedDiff(oldStr, newStr);
  return fence("diff", diff);
}

function formatMultiEditInput(pair: ToolPair): string {
  const input = pair.input as Record<string, unknown>;
  const edits = Array.isArray(input.edits)
    ? (input.edits as Array<Record<string, unknown>>)
    : [];
  if (edits.length === 0) return "";
  const diffs = edits.map((e) => {
    const o = typeof e.old_text === "string" ? e.old_text : "";
    const n = typeof e.new_text === "string" ? e.new_text : "";
    return unifiedDiff(o, n);
  });
  return fence("diff", diffs.join("\n\n---\n\n"));
}

function formatToolResult(pair: ToolPair): string {
  if (!pair.result) return "";
  // Canonical-block tool_result content is a pre-flattened string;
  // missing content just means "no output to show".
  const body = pair.result.content ?? "";
  if (!body) return "";
  const truncated =
    body.length > 1500 ? `${body.slice(0, 1500)}\n… (${body.length} chars total)` : body;
  const label = pair.isError ? "Result (error)" : "Result";
  return `_${label}_\n\n${fence("", truncated)}`;
}

function unifiedDiff(oldStr: string, newStr: string): string {
  // Cheap line-diff. Not a true Myers diff — good enough for a
  // copy-as-markdown visual. Marks the two sides with -/+ so the
  // ```diff``` fence colours them.
  const oldLines = oldStr.split("\n");
  const newLines = newStr.split("\n");
  const marked = [
    ...oldLines.map((l) => `- ${l}`),
    ...newLines.map((l) => `+ ${l}`),
  ];
  return marked.join("\n");
}

function fence(lang: string, body: string): string {
  // If the body itself contains triple-backticks, bump the fence to
  // four to avoid accidentally closing early.
  const fenceChars = body.includes("```") ? "````" : "```";
  return `${fenceChars}${lang}\n${body}\n${fenceChars}`;
}
