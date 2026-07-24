import { Fragment, useCallback, useMemo } from "react";

import { appCommands } from "../../state/AppCommands";
import "./FileRefText.css";

/** Matches a repo-relative `path/to/file.ext:line[:col]` reference, the form
 * agents use in plan prose. The path must carry a file extension (so `TODO:12`
 * or a bare `3:14` never matches), and the lookbehind rejects references glued
 * onto a longer token or a URL scheme (`http://host:8080`). */
const FILE_REF =
  // eslint-disable-next-line no-useless-escape
  /(?<![\w@:/.\-])((?:[\w.+\-]+\/)*[\w.+\-]+\.[A-Za-z][\w]*):(\d{1,7})(?::(\d{1,7}))?\b/g;

interface RefToken {
  text: string;
  path: string;
  line: number;
}

type Segment = string | RefToken;

function tokenize(text: string): Segment[] {
  const segments: Segment[] = [];
  let last = 0;
  // `matchAll` clones the regex state per call, so the shared /g literal is
  // safe to reuse across renders.
  for (const match of text.matchAll(FILE_REF)) {
    const index = match.index ?? 0;
    if (index > last) segments.push(text.slice(last, index));
    segments.push({
      text: match[0],
      path: match[1]!,
      line: Number(match[2]),
    });
    last = index + match[0].length;
  }
  if (last < text.length) segments.push(text.slice(last));
  return segments;
}

/** Renders freeform text, turning `path:line` references into clickable links
 * that open the file and reveal the line. Non-reference text is passed through
 * untouched. */
export function FileRefText({
  text,
  repo,
  workspaceId,
}: {
  text: string;
  repo: string;
  workspaceId?: string;
}) {
  const segments = useMemo(() => tokenize(text), [text]);
  return (
    <>
      {segments.map((segment, i) =>
        typeof segment === "string" ? (
          <Fragment key={i}>{segment}</Fragment>
        ) : (
          <FileRefLink
            key={i}
            repo={repo}
            workspaceId={workspaceId}
            path={segment.path}
            line={segment.line}
            label={segment.text}
          />
        ),
      )}
    </>
  );
}

function FileRefLink({
  repo,
  workspaceId,
  path,
  line,
  label,
}: {
  repo: string;
  workspaceId?: string;
  path: string;
  line: number;
  label: string;
}) {
  const open = useCallback(() => {
    appCommands.openFile({ repo, path, workspaceId, line });
  }, [line, path, repo, workspaceId]);
  return (
    <button
      type="button"
      className="file-ref"
      onClick={open}
      title={`Open ${path}:${line}`}
    >
      {label}
    </button>
  );
}
