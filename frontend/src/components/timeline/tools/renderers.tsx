import "./renderers.css";
import type { TimelineFileTouch } from "../../../api/types";
import { useRepos } from "../../../state/RepoStore";
import type { Maybe } from "../../../lib/types";
import { buildWorkspaceFileMenuItems } from "../../common/fileContextMenu";
import {
  contextMenuTriggerProps,
  useContextMenu,
} from "../../common/contextMenuStore";
import { InlineCodeDiff } from "./inlineCodeDiff";
import { UnifiedDiff } from "./unifiedDiff";

export interface ToolUseSummary {
  id?: string;
  name?: string;
  operationType?: string | null;
  input?: unknown;
  resultPayload?: unknown;
  fileTouches?: TimelineFileTouch[];
}

export function ToolCallRenderer({ tool }: { tool: ToolUseSummary }) {
  const input = record(tool.input);
  const operationType = tool.operationType ?? tool.name;
  const fileTouches = tool.fileTouches ?? [];

  // Agent-agnostic: any tool whose input canonicalised to `file_edits`
  // renders through one path. Claude Edit / MultiEdit and codex
  // apply_patch all land here, same shape.
  if (Array.isArray(input.file_edits)) {
    return <FileEditRenderer input={input} fileTouches={fileTouches} />;
  }

  switch (operationType) {
    case "write":
      return <WriteRenderer input={input} fileTouches={fileTouches} />;
    case "bash":
    case "exec":
    case "exec_command":
      return <BashRenderer input={input} fileTouches={fileTouches} />;
    case "read":
      return <ReadRenderer input={input} fileTouches={fileTouches} />;
    case "grep":
      return <GrepRenderer input={input} fileTouches={fileTouches} />;
    case "glob":
      return <GlobRenderer input={input} fileTouches={fileTouches} />;
    case "task":
      return <TaskRenderer input={input} fileTouches={fileTouches} />;
    case "todo_write":
      return <TodoRenderer input={input} fileTouches={fileTouches} />;
    case "web_fetch":
    case "web_search":
      return <WebRenderer input={input} name={operationType} fileTouches={fileTouches} />;
    default:
      return <GenericRenderer input={input} fileTouches={fileTouches} />;
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

function FileTouchList({ touches }: { touches: TimelineFileTouch[] }) {
  if (touches.length === 0) return null;
  return (
    <div className="tr-files">
      {touches.map((touch) => (
        <FileTouchRow
          key={`${touch.repo}:${touch.path}:${touch.touch_kind}`}
          touch={touch}
        />
      ))}
    </div>
  );
}

function FileTouchRow({ touch }: { touch: TimelineFileTouch }) {
  const repoState = useRepos((store) => store.repos[touch.repo]);
  const dirty = repoState?.git?.dirty_by_path[touch.path];
  const diff = repoState?.git?.diff_stats_by_path[touch.path];
  const openCtx = useContextMenu((store) => store.open);
  const triggerProps = contextMenuTriggerProps(openCtx, () =>
    buildWorkspaceFileMenuItems({
      repo: touch.repo,
      path: touch.path,
      dirty,
      copyText: `${touch.repo}:${touch.path}`,
    }),
  );

  return (
    <div
      className="tr-file"
      aria-label="File actions"
      {...triggerProps}
    >
      <span className="tr-file__meta">
        <span className="tr-file__kind">{touch.touch_kind}</span>
        {touch.is_write && <span className="tr-file__write">write</span>}
      </span>
      <code className="tr-file__path">{touch.repo}:{touch.path}</code>
      {diff && (
        <span className="tr-file__diffstat">
          +{diff.additions} -{diff.deletions}
        </span>
      )}
    </div>
  );
}

interface FileEditEntry {
  path?: unknown;
  old_path?: unknown;
  operation?: unknown;
  in_out?: { old_text?: unknown; new_text?: unknown };
  diff?: unknown;
  replace_all?: unknown;
}

// Groups contiguous file_edits by path so a single file's N in_out
// entries render under one file header.
interface FileEditGroup {
  path?: string;
  old_path?: string;
  operation: string;
  entries: FileEditEntry[];
}

function groupFileEdits(entries: FileEditEntry[]): FileEditGroup[] {
  const groups: FileEditGroup[] = [];
  for (const entry of entries) {
    const path = str(entry.path);
    const oldPath = str(entry.old_path);
    const operation = str(entry.operation) ?? "update";
    const last = groups[groups.length - 1];
    if (
      last
      && last.path === path
      && last.old_path === oldPath
      && last.operation === operation
    ) {
      last.entries.push(entry);
    } else {
      groups.push({ path, old_path: oldPath, operation, entries: [entry] });
    }
  }
  return groups;
}

function FileEditRenderer({
  input,
  fileTouches,
}: {
  input: Record<string, unknown>;
  fileTouches: TimelineFileTouch[];
}) {
  const entries = Array.isArray(input.file_edits)
    ? (input.file_edits as FileEditEntry[])
    : [];
  if (entries.length === 0) {
    return (
      <div className="tr tr--edit">
        <div className="tr-muted">file edit · nothing to show</div>
        <FileTouchList touches={fileTouches} />
      </div>
    );
  }
  const groups = groupFileEdits(entries);
  return (
    <div className="tr tr--edit">
      {groups.map((group, i) => (
        <FileEditGroupBlock key={`${group.path ?? "f"}-${i}`} group={group} />
      ))}
      <FileTouchList touches={fileTouches} />
    </div>
  );
}

function FileEditGroupBlock({ group }: { group: FileEditGroup }) {
  const { path, old_path, operation, entries } = group;
  const visible = entries.slice(0, 5);
  const overflow = entries.length - visible.length;
  return (
    <div className="tr-fe">
      <div className="tr-fe__header">
        <span className={`tr-fe__op tr-fe__op--${operation}`}>{operation}</span>
        <code className="tr-path__value">{path ?? "(no path)"}</code>
        {operation === "move" && old_path && (
          <span className="tr-muted">
            from <code>{old_path}</code>
          </span>
        )}
        {entries.length > 1 && (
          <span className="tr-muted">
            {entries.length} edits
          </span>
        )}
      </div>
      {visible.map((entry, i) => (
        <FileEditBody
          key={i}
          entry={entry}
          compact={entries.length > 1}
        />
      ))}
      {overflow > 0 && (
        <div className="tr-muted">… {overflow} more</div>
      )}
    </div>
  );
}

function FileEditBody({
  entry,
  compact,
}: {
  entry: FileEditEntry;
  compact: boolean;
}) {
  const inOut = isRecord(entry.in_out) ? entry.in_out : null;
  if (inOut) {
    return (
      <>
        <InlineCodeDiff
          oldText={str(inOut.old_text)}
          newText={str(inOut.new_text)}
          compact={compact}
        />
        {entry.replace_all && <div className="tr-flag">replace_all</div>}
      </>
    );
  }
  const diff = str(entry.diff);
  if (diff) {
    return <UnifiedDiff diff={diff} compact={compact} />;
  }
  return null;
}

function WriteRenderer({
  input,
  fileTouches,
}: {
  input: Record<string, unknown>;
  fileTouches: TimelineFileTouch[];
}) {
  const file = str(input.path);
  const content = str(input.content);
  return (
    <div className="tr tr--write">
      <PathLine label="write" value={file} />
      {content && <pre className="tr-code tr-code--added">{preview(content, 40)}</pre>}
      <FileTouchList touches={fileTouches} />
    </div>
  );
}

function BashRenderer({
  input,
  fileTouches,
}: {
  input: Record<string, unknown>;
  fileTouches: TimelineFileTouch[];
}) {
  const command = str(input.command) ?? str(input.cmd);
  const description = str(input.description);
  // Codex code-mode exec keeps the original JS snippet under `code`;
  // fall back to it when no shell command could be extracted.
  const code = str(input.code);
  return (
    <div className="tr tr--bash">
      {description && <div className="tr-desc">{description}</div>}
      {command != null || code == null ? (
        <pre className="tr-code tr-code--cmd">{"$ "}{command ?? ""}</pre>
      ) : (
        <pre className="tr-code">{code}</pre>
      )}
      <FileTouchList touches={fileTouches} />
    </div>
  );
}

function ReadRenderer({
  input,
  fileTouches,
}: {
  input: Record<string, unknown>;
  fileTouches: TimelineFileTouch[];
}) {
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
      <FileTouchList touches={fileTouches} />
    </div>
  );
}

function GrepRenderer({
  input,
  fileTouches,
}: {
  input: Record<string, unknown>;
  fileTouches: TimelineFileTouch[];
}) {
  const pattern = str(input.pattern);
  const path = str(input.path) ?? str(input.glob);
  const mode = str(input.output_mode) ?? "files_with_matches";
  return (
    <div className="tr tr--grep">
      <div>
        <span className="tr-kw">grep</span> <code className="tr-inline">{pattern}</code>
      </div>
      {path && <PathLine label="in" value={path} />}
      <div className="tr-muted">mode: {mode}</div>
      <FileTouchList touches={fileTouches} />
    </div>
  );
}

function GlobRenderer({
  input,
  fileTouches,
}: {
  input: Record<string, unknown>;
  fileTouches: TimelineFileTouch[];
}) {
  const pattern = str(input.pattern);
  const path = str(input.path);
  return (
    <div className="tr tr--glob">
      <div>
        <span className="tr-kw">glob</span> <code className="tr-inline">{pattern}</code>
      </div>
      {path && <PathLine label="in" value={path} />}
      <FileTouchList touches={fileTouches} />
    </div>
  );
}

function TaskRenderer({
  input,
  fileTouches,
}: {
  input: Record<string, unknown>;
  fileTouches: TimelineFileTouch[];
}) {
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
      <FileTouchList touches={fileTouches} />
    </div>
  );
}

function TodoRenderer({
  input,
  fileTouches,
}: {
  input: Record<string, unknown>;
  fileTouches: TimelineFileTouch[];
}) {
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
      <FileTouchList touches={fileTouches} />
    </div>
  );
}

function WebRenderer({
  input,
  name,
  fileTouches,
}: {
  input: Record<string, unknown>;
  name: string;
  fileTouches: TimelineFileTouch[];
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
      <FileTouchList touches={fileTouches} />
    </div>
  );
}

function GenericRenderer({
  input,
  fileTouches,
}: {
  input: Record<string, unknown>;
  fileTouches: TimelineFileTouch[];
}) {
  return (
    <div className="tr tr--generic">
      <pre className="tr-code tr-code--json">{JSON.stringify(input, null, 2)}</pre>
      <FileTouchList touches={fileTouches} />
    </div>
  );
}

function str(v: unknown): Maybe<string> {
  return typeof v === "string" ? v : undefined;
}

function num(v: unknown): Maybe<number> {
  return typeof v === "number" ? v : undefined;
}

function record(v: unknown): Record<string, unknown> {
  return v && typeof v === "object" && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : {};
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === "object" && !Array.isArray(v);
}

function preview(text: string, maxLines: number): string {
  const lines = text.split("\n");
  if (lines.length <= maxLines) return text;
  return `${lines.slice(0, maxLines).join("\n")}\n… +${lines.length - maxLines} more`;
}
