import "./renderers.css";
import type { TimelineFileTouch } from "../../../api/types";
import { useRepos } from "../../../state/RepoStore";
import type { Maybe } from "../../../lib/types";
import { buildWorkspaceFileMenuItems } from "../../common/fileContextMenu";
import {
  contextMenuHandler,
  useContextMenu,
} from "../../common/ContextMenu";

export interface ToolUseSummary {
  id?: string;
  name?: string;
  operationType?: string | null;
  input?: unknown;
  fileTouches?: TimelineFileTouch[];
}

export function ToolCallRenderer({ tool }: { tool: ToolUseSummary }) {
  const input = (tool.input ?? {}) as Record<string, unknown>;
  const operationType = tool.operationType ?? tool.name;
  const fileTouches = tool.fileTouches ?? [];

  switch (operationType) {
    case "edit":
      return <EditRenderer input={input} fileTouches={fileTouches} variant="edit" />;
    case "write":
      return <WriteRenderer input={input} fileTouches={fileTouches} />;
    case "multi_edit":
      return <MultiEditRenderer input={input} fileTouches={fileTouches} />;
    case "bash":
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
  const onContextMenu = contextMenuHandler(openCtx, () =>
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
      onContextMenu={onContextMenu}
      title="Right-click for file actions"
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

function EditRenderer({
  input,
  fileTouches,
  variant,
}: {
  input: Record<string, unknown>;
  fileTouches: TimelineFileTouch[];
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
      <FileTouchList touches={fileTouches} />
    </div>
  );
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

function MultiEditRenderer({
  input,
  fileTouches,
}: {
  input: Record<string, unknown>;
  fileTouches: TimelineFileTouch[];
}) {
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
      {edits.length > 5 && <div className="tr-muted">… {edits.length - 5} more</div>}
      <FileTouchList touches={fileTouches} />
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
      {oldStr && <pre className="tr-code tr-code--removed">{preview(oldStr, 30)}</pre>}
      {newStr && <pre className="tr-code tr-code--added">{preview(newStr, 30)}</pre>}
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
  return (
    <div className="tr tr--bash">
      {description && <div className="tr-desc">{description}</div>}
      <pre className="tr-code tr-code--cmd">{"$ "}{command ?? ""}</pre>
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

function preview(text: string, maxLines: number): string {
  const lines = text.split("\n");
  if (lines.length <= maxLines) return text;
  return `${lines.slice(0, maxLines).join("\n")}\n… +${lines.length - maxLines} more`;
}
