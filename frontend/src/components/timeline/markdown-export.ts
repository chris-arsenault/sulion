import type { TimelineAssistantItem } from "../../api/types";
import type { ToolPair, Turn } from "./grouping";

/** OpenAI appends memory-citation markup to assistant text
 * (`<oai-mem-citation>…</oai-mem-citation>`). It's provenance metadata,
 * not conversation — drop it from every rendered or exported surface. */
export function stripAssistantAnnotations(text: string): string {
  if (!text.includes("<oai-mem-citation>")) return text;
  return text
    .replace(/<oai-mem-citation>[\s\S]*?<\/oai-mem-citation>/g, "")
    .trimEnd();
}

export function formatTurn(turn: Turn): string {
  return turn.markdown;
}

export function formatAssistantText(items: TimelineAssistantItem[]): string {
  return items
    .flatMap((item) =>
      item.kind === "text" ? [stripAssistantAnnotations(item.text)] : [],
    )
    .join("\n\n")
    .trim();
}

export function formatAssistantItems(
  items: TimelineAssistantItem[],
  pairById: Map<string, ToolPair>,
): string {
  return items
    .flatMap((item) => {
      if (item.kind === "text") {
        return [stripAssistantAnnotations(item.text).trim()].filter(Boolean);
      }
      const pair = pairById.get(item.pair_id);
      return pair ? [formatToolPair(pair)] : [];
    })
    .filter((part) => part.length > 0)
    .join("\n\n");
}

export function formatToolPair(pair: ToolPair): string {
  const header = `**Tool:** \`${toolType(pair)}\`${toolOneLine(pair)}`;
  const inputBlock = formatToolInput(pair);
  const resultBlock = formatToolResult(pair);
  const status = pair.is_pending ? " _(pending)_" : pair.is_error ? " _(error)_" : "";
  return [`${header}${status}`, inputBlock, resultBlock].filter(Boolean).join("\n\n");
}

function toolOneLine(pair: ToolPair): string {
  const input = (pair.input ?? {}) as Record<string, unknown>;
  const pick = (key: string) =>
    typeof input[key] === "string" ? (input[key] as string) : undefined;
  const command = pick("command") ?? pick("cmd");
  if (command) return ` \`${command.slice(0, 160)}\``;
  let summary = "";
  switch (toolType(pair)) {
    case "edit":
    case "write":
    case "multi_edit":
    case "read":
      summary = pick("path") ?? "";
      break;
    case "bash":
    case "exec_command":
      summary = pick("command") ?? pick("cmd") ?? "";
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
  if (toolType(pair) === "edit" || toolType(pair) === "write") {
    return formatEditInput(pair);
  }
  if (toolType(pair) === "multi_edit") {
    return formatMultiEditInput(pair);
  }
  const cmd =
    typeof (input as { command?: unknown })?.command === "string"
      ? ((input as { command: string }).command)
      : typeof (input as { cmd?: unknown })?.cmd === "string"
        ? ((input as { cmd: string }).cmd)
        : "";
  if (cmd) return fence("bash", cmd);
  if (toolType(pair) === "todo_write") {
    const todos = (input as { todos?: Array<{ status?: string; content?: string }> })
      ?.todos;
    if (!Array.isArray(todos) || todos.length === 0) return "";
    const lines = todos.map((todo) => {
      const box =
        todo.status === "completed" ? "[x]" : todo.status === "in_progress" ? "[~]" : "[ ]";
      return `- ${box} ${todo.content ?? ""}`;
    });
    return lines.join("\n");
  }
  return fence("json", JSON.stringify(input ?? {}, null, 2));
}

function toolType(pair: ToolPair): string {
  return pair.operation_type ?? pair.name;
}

function formatEditInput(pair: ToolPair): string {
  const input = structuredEditInput(pair);
  const oldText = typeof input.old_text === "string" ? input.old_text : "";
  const newText =
    typeof input.new_text === "string"
      ? input.new_text
      : typeof input.content === "string"
        ? input.content
        : "";
  if (!oldText && !newText) return "";
  return fence("diff", unifiedDiff(oldText, newText));
}

function formatMultiEditInput(pair: ToolPair): string {
  const input = structuredEditInput(pair);
  const edits = Array.isArray(input.edits)
    ? (input.edits as Array<Record<string, unknown>>)
    : [];
  if (edits.length === 0) return "";
  const diffs = edits.map((edit) => {
    const oldText = typeof edit.old_text === "string" ? edit.old_text : "";
    const newText = typeof edit.new_text === "string" ? edit.new_text : "";
    return unifiedDiff(oldText, newText);
  });
  return fence("diff", diffs.join("\n\n---\n\n"));
}

function structuredEditInput(pair: ToolPair): Record<string, unknown> {
  const payload = pair.result?.payload;
  if (
    payload &&
    typeof payload === "object" &&
    !Array.isArray(payload) &&
    (toolType(pair) === "edit" || toolType(pair) === "multi_edit")
  ) {
    return payload as Record<string, unknown>;
  }
  return (pair.input ?? {}) as Record<string, unknown>;
}

function formatToolResult(pair: ToolPair): string {
  if (!pair.result) return "";
  const body = pair.result.content ?? "";
  if (!body) return "";
  const truncated =
    body.length > 1500 ? `${body.slice(0, 1500)}\n… (${body.length} chars total)` : body;
  const label = pair.is_error ? "Result (error)" : "Result";
  return `_${label}_\n\n${fence("", truncated)}`;
}

function unifiedDiff(oldStr: string, newStr: string): string {
  return [
    ...oldStr.split("\n").map((line) => `- ${line}`),
    ...newStr.split("\n").map((line) => `+ ${line}`),
  ].join("\n");
}

function fence(lang: string, body: string): string {
  const fenceChars = body.includes("```") ? "````" : "```";
  return `${fenceChars}${lang}\n${body}\n${fenceChars}`;
}
