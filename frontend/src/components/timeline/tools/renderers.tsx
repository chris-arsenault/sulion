// Tool-aware renderers for tool_use blocks. Each renderer knows the
// shape of one tool's `input` and lays it out in a way that's easier to
// scan than raw JSON. Unknown tools fall through to the generic block.
//
// File-path cells are clickable when the path parses as an absolute
// repo-rooted path (`/home/dev/repos/<name>/<rel>`): clicking opens a
// FileTab. When the file is currently dirty according to RepoStore, a
// small "diff" affordance opens a DiffTab for that file. Bash
// commands linkify obvious repo-rooted path arguments by the same
// matcher.

import "./renderers.css";
import { useRepos } from "../../../state/RepoStore";
import { useTabs } from "../../../state/TabStore";
import type { Maybe } from "../../../lib/types";

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

// ─── repo-rooted path helpers ────────────────────────────────────────

const REPO_ROOT_PATTERN = /^\/home\/dev\/repos\/([^/]+)\/(.*)$/;

function parseRepoRootedPath(
  abs: string | undefined,
): { repo: string; rel: string } | null {
  if (!abs) return null;
  const m = abs.match(REPO_ROOT_PATTERN);
  if (!m || !m[1]) return null;
  return { repo: m[1], rel: m[2] ?? "" };
}

/** Display for a file path. When the value is a repo-rooted absolute
 * path we render it as a clickable chip that opens a FileTab; if the
 * file is currently dirty in the repo store we append a "diff" chip
 * that opens a DiffTab. */
function PathLine({ label, value }: { label: string; value?: string }) {
  if (!value) return null;
  const parsed = parseRepoRootedPath(value);
  return (
    <div className="tr-path">
      <span className="tr-path__label">{label}</span>
      {parsed ? (
        <ClickablePath repo={parsed.repo} rel={parsed.rel} display={value} />
      ) : (
        <code className="tr-path__value">{value}</code>
      )}
    </div>
  );
}

function ClickablePath({
  repo,
  rel,
  display,
}: {
  repo: string;
  rel: string;
  display: string;
}) {
  const openTab = useTabs((store) => store.openTab);
  const repos = useRepos((store) => store.repos);
  const dirty = repos[repo]?.git?.dirty_by_path[rel];
  return (
    <span className="tr-path__clickable">
      <button
        type="button"
        className="tr-path__link"
        onClick={() => openTab({ kind: "file", repo, path: rel })}
        title={`Open ${display}`}
        aria-label={`Open ${display}`}
      >
        <code className="tr-path__value">{display}</code>
      </button>
      {dirty && (
        <button
          type="button"
          className="tr-path__diff"
          onClick={() => openTab({ kind: "diff", repo, path: rel })}
          title={`Open diff (${dirty.trim()})`}
        >
          {dirty.trim() || "•"} diff
        </button>
      )}
    </span>
  );
}

function InlinePath({
  repo,
  rel,
  display,
}: {
  repo: string;
  rel: string;
  display: string;
}) {
  const openTab = useTabs((store) => store.openTab);
  return (
    <button
      type="button"
      className="tr-path__inline"
      onClick={() => openTab({ kind: "file", repo, path: rel })}
      title={`Open ${display}`}
      aria-label={`Open ${display}`}
    >
      {display}
    </button>
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
  const file = str(input.path);
  const oldStr = str(input.old_text);
  const newStr = str(input.new_text);
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
  const file = str(input.path);
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
  const file = str(input.path);
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
          oldStr={str(e.old_text)}
          newStr={str(e.new_text)}
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
      <pre className="tr-code tr-code--cmd">
        {"$ "}
        {command ? <LinkifiedCommand command={command} /> : ""}
      </pre>
    </div>
  );
}

/** Walk a shell command string and wrap any token that looks like a
 * repo-rooted absolute path (`/home/dev/repos/<name>/<rel>`) in a
 * clickable span. Conservative: we only match whole tokens (whitespace-
 * delimited, with the standard repo-root prefix) so we don't
 * false-positive on quoted strings or shell interpolations. */
function LinkifiedCommand({ command }: { command: string }) {
  // Capture one character of boundary context so we can preserve the
  // surrounding whitespace / punctuation exactly.
  const parts = command.split(/(\/home\/dev\/repos\/[^\s'"`:;]+)/g);
  return (
    <>
      {parts.map((part, i) => {
        const parsed = parseRepoRootedPath(part);
        if (!parsed) return <span key={i}>{part}</span>;
        return (
          <InlinePath
            key={i}
            repo={parsed.repo}
            rel={parsed.rel}
            display={part}
          />
        );
      })}
    </>
  );
}

function ReadRenderer({ input }: { input: Record<string, unknown> }) {
  const file = str(input.path);
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
  const agent = str(input.agent);
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
