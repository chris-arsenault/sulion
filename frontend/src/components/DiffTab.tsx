// Diff viewer. Renders `git diff HEAD` as a per-file split list with
// ±-coloured lines. Per-file staging buttons hit /api/repos/:name/git/stage.
// No per-hunk staging for v1 — file-level is enough for "quick review
// and commit" flows, which is the whole pitch.

import { useCallback, useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { getRepoDiff, stageRepoPath } from "../api/client";
import { useRepos } from "../state/RepoStore";
import type { FileDiff, DiffLine } from "../workers/diffParser.worker";
import "./DiffTab.css";

export function DiffTab({ repo, path }: { repo: string; path?: string }) {
  const [fileDiffs, setFileDiffs] = useState<FileDiff[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const { dirtyMap, refreshRepo } = useRepos(
    useShallow((store) => ({
      dirtyMap: store.repos[repo]?.git?.dirty_by_path ?? {},
      refreshRepo: store.refresh,
    })),
  );
  const workerRef = useRef<Worker | null>(null);

  useEffect(() => {
    workerRef.current = new Worker(
      new URL("../workers/diffParser.worker.ts", import.meta.url),
      { type: "module" },
    );
    workerRef.current.onmessage = (ev: MessageEvent<{ files: FileDiff[] }>) => {
      setFileDiffs(ev.data.files);
    };
    return () => {
      workerRef.current?.terminate();
      workerRef.current = null;
    };
  }, []);

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    getRepoDiff(repo, path)
      .then((r) => {
        workerRef.current?.postMessage({ raw: r.diff });
      })
      .catch((err) => setError(err instanceof Error ? err.message : "load failed"))
      .finally(() => setLoading(false));
  }, [repo, path]);

  useEffect(load, [load]);

  const onStage = async (p: string, stage: boolean) => {
    try {
      await stageRepoPath(repo, p, stage);
      refreshRepo(repo);
      load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "stage failed");
    }
  };

  return (
    <div className="dt">
      <div className="dt__header">
        <span className="dt__title">
          {path ? `diff · ${path}` : `${repo} · full diff`}
        </span>
        <button type="button" className="dt__refresh" onClick={load}>
          ↻ refresh
        </button>
      </div>
      {error && <div className="dt__err">{error}</div>}
      {loading && fileDiffs.length === 0 && (
        <div className="dt__muted">loading…</div>
      )}
      {!loading && fileDiffs.length === 0 && !error && (
        <div className="dt__muted">working tree clean.</div>
      )}
      <div className="dt__body">
        {fileDiffs.map((fd) => {
          const code = dirtyMap[fd.path] ?? "  ";
          const staged = code[0] !== " " && code[0] !== "?";
          return (
            <FileDiffView
              key={fd.path}
              diff={fd}
              statusCode={code}
              staged={staged}
              onToggleStage={() => onStage(fd.path, !staged)}
            />
          );
        })}
      </div>
    </div>
  );
}

function FileDiffView({
  diff,
  statusCode,
  staged,
  onToggleStage,
}: {
  diff: FileDiff;
  statusCode: string;
  staged: boolean;
  onToggleStage: () => void;
}) {
  const [collapsed, setCollapsed] = useState(false);
  return (
    <div className="dt__file">
      <div className="dt__file-header">
        <button
          type="button"
          className="dt__file-toggle"
          onClick={() => setCollapsed((v) => !v)}
        >
          <span className={collapsed ? "dt__chev" : "dt__chev dt__chev--open"}>
            ▸
          </span>
          <span className="dt__file-code">{statusCode.trim() || "•"}</span>
          <span className="dt__file-path">{diff.path}</span>
        </button>
        <button
          type="button"
          className={
            staged ? "dt__stage dt__stage--staged" : "dt__stage"
          }
          onClick={onToggleStage}
          title={staged ? "Unstage this file" : "Stage this file"}
        >
          {staged ? "unstage" : "stage"}
        </button>
      </div>
      {!collapsed && (
        <pre className="dt__hunks">
          {diff.lines
            .filter((l) => l.kind !== "f")
            .map((l, i) => (
              <span key={i} className={`dt__ln dt__ln--${lineClass(l.kind)}`}>
                {renderPrefix(l.kind)}
                {l.text}
                {"\n"}
              </span>
            ))}
        </pre>
      )}
    </div>
  );
}

function lineClass(kind: DiffLine["kind"]): string {
  if (kind === "+") return "add";
  if (kind === "-") return "del";
  if (kind === "@") return "hunk";
  if (kind === "i") return "idx";
  return "ctx";
}

function renderPrefix(kind: DiffLine["kind"]): string {
  if (kind === "+") return "+";
  if (kind === "-") return "-";
  if (kind === "@") return "";
  if (kind === "i") return "";
  return " ";
}
