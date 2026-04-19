// Tool-aware renderers for tool_use blocks. Each renderer knows the
// shape of one tool's `input` and lays it out in a way that's easier to
// scan than raw JSON. Unknown tools fall through to the generic block.

import "./renderers.css";

export interface ToolUseSummary {
  id?: string;
  name?: string;
  input?: unknown;
}

export function ToolCallRenderer({ tool }: { tool: ToolUseSummary }) {
  const input = (tool.input ?? {}) as Record<string, unknown>;
  // Dispatch on the canonical tool name. Agent-specific raw names are
  // mapped to canonical form by the ingester before they reach us, so
  // the same renderer handles "Read" (Claude Code) and "read_file"
  // (Codex) without caring which one it is.
  switch (tool.name) {
    case "edit":
      return <EditRenderer input={input} variant="edit" />;
    case "write":
      return <WriteRenderer input={input} />;
    case "multi_edit":
      return <MultiEditRenderer input={input} />;
    case "bash":
      return <BashRenderer input={input} />;
    case "read":
      return <ReadRenderer input={input} />;
    case "grep":
      return <GrepRenderer input={input} />;
    case "glob":
      return <GlobRenderer input={input} />;
    case "task":
      return <TaskRenderer input={input} />;
    case "todo_write":
      return <TodoRenderer input={input} />;
    case "web_fetch":
    case "web_search":
      return <WebRenderer input={input} name={tool.name} />;
    default:
      return <GenericRenderer input={input} />;
  }
}

function PathLine({ label, value }: { label: string; value?: string }) {
  if (!value) return null;
  return (
    <div className="tr-path">
      <span className="tr-path__label">{label}</span>
      <code className="tr-path__value">{value}</code>
    </div>
  );
}

function EditRenderer({
  input,
  variant,
}: {
  input: Record<string, unknown>;
  variant: "edit";
}) {
  void variant;
  const file = str(input.file_path);
  const oldStr = str(input.old_string);
  const newStr = str(input.new_string);
  const replaceAll = Boolean(input.replace_all);
  return (
    <div className="tr tr--edit">
      <PathLine label="file" value={file} />
      <DiffBlock oldStr={oldStr} newStr={newStr} />
      {replaceAll && <div className="tr-flag">replace_all</div>}
    </div>
  );
}

function WriteRenderer({ input }: { input: Record<string, unknown> }) {
  const file = str(input.file_path);
  const content = str(input.content);
  return (
    <div className="tr tr--write">
      <PathLine label="write" value={file} />
      {content && (
        <pre className="tr-code tr-code--added">
          {preview(content, 40)}
        </pre>
      )}
    </div>
  );
}

function MultiEditRenderer({ input }: { input: Record<string, unknown> }) {
  const file = str(input.file_path);
  const edits = Array.isArray(input.edits)
    ? (input.edits as Array<Record<string, unknown>>)
    : [];
  return (
    <div className="tr tr--edit">
      <PathLine label="file" value={file} />
      <div className="tr-muted">{edits.length} edit{edits.length === 1 ? "" : "s"}</div>
      {edits.slice(0, 5).map((e, i) => (
        <DiffBlock
          key={i}
          oldStr={str(e.old_string)}
          newStr={str(e.new_string)}
          compact
        />
      ))}
      {edits.length > 5 && (
        <div className="tr-muted">… {edits.length - 5} more</div>
      )}
    </div>
  );
}

function DiffBlock({
  oldStr,
  newStr,
  compact,
}: {
  oldStr?: string;
  newStr?: string;
  compact?: boolean;
}) {
  return (
    <div className={compact ? "tr-diff tr-diff--compact" : "tr-diff"}>
      {oldStr && (
        <pre className="tr-code tr-code--removed">
          {preview(oldStr, 30)}
        </pre>
      )}
      {newStr && (
        <pre className="tr-code tr-code--added">
          {preview(newStr, 30)}
        </pre>
      )}
    </div>
  );
}

function BashRenderer({ input }: { input: Record<string, unknown> }) {
  const command = str(input.command);
  const description = str(input.description);
  return (
    <div className="tr tr--bash">
      {description && <div className="tr-desc">{description}</div>}
      <pre className="tr-code tr-code--cmd">$ {command ?? ""}</pre>
    </div>
  );
}

function ReadRenderer({ input }: { input: Record<string, unknown> }) {
  const file = str(input.file_path);
  const offset = num(input.offset);
  const limit = num(input.limit);
  return (
    <div className="tr tr--read">
      <PathLine label="read" value={file} />
      {(offset != null || limit != null) && (
        <div className="tr-muted">
          {offset != null ? `from line ${offset} ` : ""}
          {limit != null ? `limit ${limit}` : ""}
        </div>
      )}
    </div>
  );
}

function GrepRenderer({ input }: { input: Record<string, unknown> }) {
  const pattern = str(input.pattern);
  const path = str(input.path) ?? str(input.glob);
  const mode = str(input.output_mode) ?? "files_with_matches";
  return (
    <div className="tr tr--grep">
      <div>
        <span className="tr-kw">grep</span>{" "}
        <code className="tr-inline">{pattern}</code>
      </div>
      {path && <PathLine label="in" value={path} />}
      <div className="tr-muted">mode: {mode}</div>
    </div>
  );
}

function GlobRenderer({ input }: { input: Record<string, unknown> }) {
  const pattern = str(input.pattern);
  const path = str(input.path);
  return (
    <div className="tr tr--glob">
      <div>
        <span className="tr-kw">glob</span>{" "}
        <code className="tr-inline">{pattern}</code>
      </div>
      {path && <PathLine label="in" value={path} />}
    </div>
  );
}

function TaskRenderer({ input }: { input: Record<string, unknown> }) {
  const agent = str(input.subagent_type);
  const description = str(input.description);
  const prompt = str(input.prompt);
  return (
    <div className="tr tr--task">
      <div>
        <span className="tr-kw">task</span>
        {agent && <span className="tr-agent"> · {agent}</span>}
      </div>
      {description && <div className="tr-desc">{description}</div>}
      {prompt && <pre className="tr-code">{preview(prompt, 20)}</pre>}
    </div>
  );
}

function TodoRenderer({ input }: { input: Record<string, unknown> }) {
  const todos = Array.isArray(input.todos)
    ? (input.todos as Array<Record<string, unknown>>)
    : [];
  return (
    <div className="tr tr--todo">
      <ul className="tr-todos">
        {todos.map((t, i) => (
          <li key={i} className={`tr-todo tr-todo--${str(t.status) ?? "pending"}`}>
            <span className="tr-todo__status">{str(t.status) ?? "pending"}</span>
            <span className="tr-todo__content">{str(t.content) ?? ""}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function WebRenderer({
  input,
  name,
}: {
  input: Record<string, unknown>;
  name: string;
}) {
  const url = str(input.url) ?? str(input.query);
  const prompt = str(input.prompt);
  return (
    <div className="tr tr--web">
      <div>
        <span className="tr-kw">{name.toLowerCase()}</span>
      </div>
      {url && <code className="tr-inline">{url}</code>}
      {prompt && <div className="tr-muted">{preview(prompt, 8)}</div>}
    </div>
  );
}

function GenericRenderer({ input }: { input: Record<string, unknown> }) {
  return (
    <pre className="tr-code tr-code--json">{JSON.stringify(input, null, 2)}</pre>
  );
}

// ─── helpers ──────────────────────────────────────────────────────────

import type { Maybe } from "../../../lib/types";

function str(v: unknown): Maybe<string> {
  return typeof v === "string" ? v : undefined;
}

function num(v: unknown): Maybe<number> {
  return typeof v === "number" ? v : undefined;
}

/** Clip to `maxLines` lines with a "…" tail when truncated. */
function preview(text: string, maxLines: number): string {
  const lines = text.split("\n");
  if (lines.length <= maxLines) return text;
  return `${lines.slice(0, maxLines).join("\n")}\n… +${lines.length - maxLines} more`;
}
