// File preview tab. Format dispatch:
//   - markdown  → rendered via the Markdown component
//   - json      → interactive tree via JsonTree (raw toggle returns <pre>)
//   - ndjson    → tree per line
//   - image/svg → <img> via the authenticated blob route, never inline HTML,
//                 because repo SVG is agent-authored and an inlined <svg> runs
//                 its own event handlers (onbegin, foreignObject/onerror)
//   - code      → Shiki syntax highlighting (off-thread worker)
//   - fallback  → <pre>
//
// Over 1 MiB the backend refuses to serve the content and the tab
// shows a truncation banner. Raw toggle in the header flips anything
// non-image to a plain <pre> view without changing the stored format.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  fetchFileBlob,
  fileRawUrl,
  getRepoFile,
  getWorkspaceFile,
} from "../api/client";
import { Icon } from "../icons";
import { Tooltip } from "./ui";
import type { FileResponse } from "../api/types";
import { useRepos } from "../state/RepoStore";
import { useTabs } from "../state/TabStore";
import { JsonTree } from "./common/JsonTree";
import { FileTracePanel } from "./FileTracePanel";
import { Markdown } from "./timeline/Markdown";
import { canToggleRaw, chooseRenderKind } from "./fileRenderKind";
import type { RenderKind } from "./fileRenderKind";
import "./FileTab.css";

export function FileTab({
  repo,
  path,
  workspaceId,
  focusLine,
  focusKey,
}: {
  repo: string;
  path: string;
  workspaceId?: string;
  focusLine?: number;
  focusKey?: string;
}) {
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
    const loadFile = workspaceId
      ? getWorkspaceFile(workspaceId, path)
      : getRepoFile(repo, path);
    loadFile
      .then((r) => {
        if (!cancelled) setData(r);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "load failed");
      });
    return () => {
      cancelled = true;
    };
  }, [repo, path, workspaceId]);

  const openDiffTab = useCallback(
    () => openTab({ kind: "diff", repo, path, workspaceId }),
    [openTab, repo, path, workspaceId],
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
        <span className="ft__path">
          {workspaceId ? "workspace · " : ""}
          {path}
        </span>
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
        <FileTracePanel repo={repo} path={path} workspaceId={workspaceId} />
        <FileBody
          data={data}
          repo={repo}
          workspaceId={workspaceId}
          kind={renderKind}
          focusLine={focusLine}
          focusKey={focusKey}
        />
      </div>
    </div>
  );
}


function FileBody({
  data,
  repo,
  workspaceId,
  kind,
  focusLine,
  focusKey,
}: {
  data: FileResponse;
  repo: string;
  workspaceId?: string;
  kind: RenderKind;
  focusLine?: number;
  focusKey?: string;
}) {
  if (kind.kind === "truncated") {
    return (
      <div className="ft__notice">
        <span className="ft__muted">
          File exceeds 1 MiB; inline preview disabled.
        </span>
        <FileDownload repo={repo} workspaceId={workspaceId} path={data.path} />
      </div>
    );
  }
  if (kind.kind === "image-binary") {
    return (
      <AuthenticatedImage
        repo={repo}
        workspaceId={workspaceId}
        path={data.path}
        alt={data.path}
      />
    );
  }
  if (kind.kind === "binary") {
    return (
      <div className="ft__notice">
        <span className="ft__muted">Binary file ({formatSize(data.size)}).</span>
        <FileDownload repo={repo} workspaceId={workspaceId} path={data.path} />
      </div>
    );
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
    return (
      <HighlightedCode
        lang={kind.lang}
        code={kind.code}
        focusLine={focusLine}
        focusKey={focusKey}
      />
    );
  }
  return <pre className="ft__code">{kind.code}</pre>;
}

function HighlightedCode({
  lang,
  code,
  focusLine,
  focusKey,
}: {
  lang: string;
  code: string;
  focusLine?: number;
  focusKey?: string;
}) {
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
  return <HighlightedBody html={html} focusLine={focusLine} focusKey={focusKey} />;
}

function AuthenticatedImage({
  repo,
  workspaceId,
  path,
  alt,
}: {
  repo: string;
  workspaceId?: string;
  path: string;
  alt: string;
}) {
  const [src, setSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let objectUrl: string | null = null;
    setSrc(null);
    setError(null);
    void (async () => {
      try {
        const blob = await fetchFileBlob(fileRawUrl({ repo, workspaceId }, path));
        if (cancelled) return;
        objectUrl = URL.createObjectURL(blob);
        setSrc(objectUrl);
      } catch (err) {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : "image fetch failed");
      }
    })();
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [repo, path, workspaceId]);

  if (error) return <div className="ft__muted">image preview failed: {error}</div>;
  if (!src) return <div className="ft__muted">loading image…</div>;
  return <img src={src} alt={alt} className="ft__img" />;
}

/** Downloads a file's bytes through the authenticated raw route. Used for
 * binaries the viewer can't render inline and for oversized text. Same fetch +
 * object-URL path as the image viewer, so it works on the LAN and the proxy. */
function FileDownload({
  repo,
  workspaceId,
  path,
}: {
  repo: string;
  workspaceId?: string;
  path: string;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const download = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const blob = await fetchFileBlob(fileRawUrl({ repo, workspaceId }, path));
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = path.slice(path.lastIndexOf("/") + 1) || "download";
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      URL.revokeObjectURL(url);
    } catch (err) {
      setError(err instanceof Error ? err.message : "download failed");
    } finally {
      setBusy(false);
    }
  }, [path, repo, workspaceId]);

  return (
    <span className="ft__download-row">
      <button
        type="button"
        className="ft__download"
        onClick={download}
        disabled={busy}
      >
        {busy ? "Downloading…" : "Download"}
      </button>
      {error ? <span className="ft__parse-err">{error}</span> : null}
    </span>
  );
}

function HighlightedBody({
  html,
  focusLine,
  focusKey,
}: {
  html: string;
  focusLine?: number;
  focusKey?: string;
}) {
  const inner = useMemo(() => ({ __html: html }), [html]);
  const ref = useRef<HTMLDivElement | null>(null);

  // Shiki emits one `<span class="line">` per source line. Scroll the
  // requested line into view and flash it whenever a focus request lands
  // (keyed on focusKey so the same line can be re-targeted).
  useEffect(() => {
    if (!focusLine) return;
    const container = ref.current;
    if (!container) return;
    const lines = container.querySelectorAll<HTMLElement>(".line");
    const target = lines[focusLine - 1];
    if (!target) return;
    target.scrollIntoView({ block: "center" });
    target.classList.add("ft__line--focus");
    const timer = window.setTimeout(
      () => target.classList.remove("ft__line--focus"),
      1600,
    );
    return () => {
      window.clearTimeout(timer);
      target.classList.remove("ft__line--focus");
    };
  }, [html, focusLine, focusKey]);

  return (
    <div ref={ref} className="ft__highlighted" dangerouslySetInnerHTML={inner} />
  );
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

