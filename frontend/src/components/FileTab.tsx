// File preview tab. Format dispatch:
//   - markdown  → rendered via the Markdown component
//   - json      → interactive tree via JsonTree (raw toggle returns <pre>)
//   - ndjson    → tree per line
//   - image/svg → inline
//   - code      → Shiki syntax highlighting (off-thread worker)
//   - fallback  → <pre>
//
// Over 1 MiB the backend refuses to serve the content and the tab
// shows a truncation banner. Raw toggle in the header flips anything
// non-image to a plain <pre> view without changing the stored format.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { getRepoFile } from "../api/client";
import { Icon } from "../icons";
import { Tooltip } from "./ui";
import type { FileResponse } from "../api/types";
import { useRepos } from "../state/RepoStore";
import { useTabs } from "../state/TabStore";
import { JsonTree } from "./common/JsonTree";
import { FileTracePanel } from "./FileTracePanel";
import { Markdown } from "./timeline/Markdown";
import "./FileTab.css";

export function FileTab({ repo, path }: { repo: string; path: string }) {
  const [data, setData] = useState<FileResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [raw, setRaw] = useState(false);
  const repoState = useRepos((store) => store.repos[repo]);
  const dirty = repoState?.git?.dirty_by_path[path];
  const diff = repoState?.git?.diff_stats_by_path[path];
  const openTab = useTabs((store) => store.openTab);

  useEffect(() => {
    let cancelled = false;
    setData(null);
    setError(null);
    setRaw(false);
    getRepoFile(repo, path)
      .then((r) => {
        if (!cancelled) setData(r);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "load failed");
      });
    return () => {
      cancelled = true;
    };
  }, [repo, path]);

  const openDiffTab = useCallback(
    () => openTab({ kind: "diff", repo, path }),
    [openTab, repo, path],
  );
  const toggleRaw = useCallback(() => setRaw((v) => !v), []);

  if (error) {
    return (
      <div className="ft ft--err">
        <div className="ft__header">
          <span className="ft__path">{path}</span>
        </div>
        <div className="ft__body">error: {error}</div>
      </div>
    );
  }

  if (!data) {
    return (
      <div className="ft">
        <div className="ft__header">
          <span className="ft__path">{path}</span>
        </div>
        <div className="ft__body ft__body--muted">loading…</div>
      </div>
    );
  }

  const renderKind = chooseRenderKind(data, raw);

  return (
    <div className="ft" data-testid="file-tab">
      <div className="ft__header">
        <span className="ft__path">{path}</span>
        <span className="ft__meta">
          {formatSize(data.size)} · {data.mime}
          {data.binary ? " · binary" : ""}
          {data.truncated ? " · truncated" : ""}
        </span>
        {diff && (
          <span className="ft__churn">
            +{diff.additions} -{diff.deletions}
          </span>
        )}
        {dirty && (
          <Tooltip label={`Open diff (${dirty.trim()})`}>
            <button
              type="button"
              className="ft__diff-btn"
              onClick={openDiffTab}
            >
              <Icon name="diff" size={12} />
              <span className="tabular">{dirty.trim() || "•"}</span>
              <span>diff</span>
            </button>
          </Tooltip>
        )}
        {canToggleRaw(data) && (
          <Tooltip
            label={
              raw
                ? "Switch back to the formatted view"
                : "Switch to a raw monospace view"
            }
          >
            <button
              type="button"
              className="ft__raw-btn"
              aria-pressed={raw}
              onClick={toggleRaw}
            >
              {raw ? "formatted" : "raw"}
            </button>
          </Tooltip>
        )}
      </div>
      <div className="ft__body">
        <FileTracePanel repo={repo} path={path} />
        <FileBody data={data} repo={repo} kind={renderKind} />
      </div>
    </div>
  );
}

type RenderKind =
  | { kind: "truncated" }
  | { kind: "image-binary"; src: string }
  | { kind: "image-svg"; svg: string }
  | { kind: "binary" }
  | { kind: "markdown"; source: string }
  | { kind: "json"; value: unknown; parseError?: string }
  | { kind: "ndjson"; entries: Array<{ line: number; value: unknown; parseError?: string }> }
  | { kind: "code"; lang: string; code: string }
  | { kind: "raw"; code: string };

function chooseRenderKind(data: FileResponse, raw: boolean): RenderKind {
  if (data.truncated) return { kind: "truncated" };
  if (data.binary) {
    if (data.mime.startsWith("image/")) {
      return {
        kind: "image-binary",
        src: `/api/repos/${encodeURIComponent("_")}`, // placeholder — see rendering branch below
      };
    }
    return { kind: "binary" };
  }
  // Text. SVG first (text-sniffed).
  if (data.mime === "image/svg+xml" && data.content) {
    return { kind: "image-svg", svg: data.content };
  }
  if (raw) return { kind: "raw", code: data.content ?? "" };

  const content = data.content ?? "";
  if (data.mime === "text/markdown" && content) {
    return { kind: "markdown", source: content };
  }
  const ext = extensionOf(data.path);
  if (ext === "json" || data.mime === "application/json") {
    try {
      return { kind: "json", value: JSON.parse(content) };
    } catch (err) {
      return {
        kind: "json",
        value: content,
        parseError: err instanceof Error ? err.message : "parse failed",
      };
    }
  }
  if (ext === "ndjson" || ext === "jsonl") {
    const entries = content.split("\n").flatMap((line, i) => {
      const t = line.trim();
      if (!t) return [];
      try {
        return [{ line: i + 1, value: JSON.parse(t) }];
      } catch (err) {
        return [
          {
            line: i + 1,
            value: t,
            parseError: err instanceof Error ? err.message : "parse failed",
          },
        ];
      }
    });
    return { kind: "ndjson", entries };
  }
  const lang = shikiLangFor(ext);
  if (lang) return { kind: "code", lang, code: content };
  return { kind: "raw", code: content };
}

function canToggleRaw(data: FileResponse): boolean {
  if (data.truncated) return false;
  if (data.binary) return false;
  if (data.mime === "image/svg+xml") return false;
  return true;
}

function FileBody({
  data,
  repo,
  kind,
}: {
  data: FileResponse;
  repo: string;
  kind: RenderKind;
}) {
  if (kind.kind === "truncated") {
    return (
      <div className="ft__muted">
        File exceeds 1 MiB; preview disabled. Use the terminal to inspect it
        directly.
      </div>
    );
  }
  if (kind.kind === "image-binary") {
    const src = `/api/repos/${encodeURIComponent(repo)}/file?path=${encodeURIComponent(
      data.path,
    )}&raw=1`;
    return <img src={src} alt={data.path} className="ft__img" />;
  }
  if (kind.kind === "binary") {
    return (
      <div className="ft__muted">Binary file ({formatSize(data.size)}).</div>
    );
  }
  if (kind.kind === "image-svg") {
    return <SvgBody svg={kind.svg} />;
  }
  if (kind.kind === "markdown") {
    return (
      <div className="ft__md">
        <Markdown source={kind.source} />
      </div>
    );
  }
  if (kind.kind === "json") {
    return (
      <>
        {kind.parseError && (
          <div className="ft__parse-err">invalid JSON: {kind.parseError}</div>
        )}
        <JsonTree value={kind.value} />
      </>
    );
  }
  if (kind.kind === "ndjson") {
    return (
      <div className="ft__ndjson">
        {kind.entries.map((e) => (
          <div key={e.line} className="ft__ndjson-row">
            <span className="ft__ndjson-line">{e.line}</span>
            {e.parseError && (
              <span className="ft__parse-err">invalid JSON: {e.parseError}</span>
            )}
            <JsonTree value={e.value} depthLimit={1} />
          </div>
        ))}
      </div>
    );
  }
  if (kind.kind === "code") {
    return <HighlightedCode lang={kind.lang} code={kind.code} />;
  }
  return <pre className="ft__code">{kind.code}</pre>;
}

function HighlightedCode({ lang, code }: { lang: string; code: string }) {
  const [html, setHtml] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const reqId = useRef(0);

  useEffect(() => {
    const id = ++reqId.current;
    setHtml(null);
    setFailed(false);
    const w = new Worker(
      new URL("../workers/syntaxHighlighter.worker.ts", import.meta.url),
      { type: "module" },
    );
    w.onmessage = (ev: MessageEvent<
      | { kind: "highlighted"; id: number; html: string }
      | { kind: "error"; id: number; message: string }
      | { kind: "ready" }
    >) => {
      const m = ev.data;
      if (m.kind === "ready") return;
      if (m.id !== id) return; // stale response
      if (m.kind === "highlighted") setHtml(m.html);
      else setFailed(true);
    };
    w.postMessage({ kind: "highlight", lang, code, id });
    return () => {
      w.terminate();
    };
  }, [lang, code]);

  if (failed) {
    return <pre className="ft__code">{code}</pre>;
  }
  if (html == null) {
    return (
      <pre className="ft__code ft__code--loading">{code}</pre>
    );
  }
  return <HighlightedBody html={html} />;
}

function SvgBody({ svg }: { svg: string }) {
  const html = useMemo(() => ({ __html: svg }), [svg]);
  return <div className="ft__svg" dangerouslySetInnerHTML={html} />;
}

function HighlightedBody({ html }: { html: string }) {
  const inner = useMemo(() => ({ __html: html }), [html]);
  return <div className="ft__highlighted" dangerouslySetInnerHTML={inner} />;
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

function extensionOf(path: string): string {
  const base = path.slice(path.lastIndexOf("/") + 1);
  const i = base.lastIndexOf(".");
  return i === -1 ? "" : base.slice(i + 1).toLowerCase();
}

function shikiLangFor(ext: string): string | null {
  switch (ext) {
    case "rs":
      return "rust";
    case "ts":
      return "typescript";
    case "tsx":
      return "tsx";
    case "js":
      return "javascript";
    case "jsx":
      return "jsx";
    case "py":
      return "python";
    case "go":
      return "go";
    case "java":
      return "java";
    case "c":
    case "h":
      return "c";
    case "cpp":
    case "hpp":
    case "cc":
      return "cpp";
    case "sh":
    case "bash":
      return "bash";
    case "toml":
      return "toml";
    case "yaml":
    case "yml":
      return "yaml";
    case "sql":
      return "sql";
    case "css":
      return "css";
    case "scss":
      return "scss";
    case "html":
    case "htm":
      return "html";
    case "patch":
    case "diff":
      return "diff";
    default:
      return null;
  }
}
