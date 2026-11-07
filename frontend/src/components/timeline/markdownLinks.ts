// Classifies markdown link hrefs in timeline text. Agents write links
// like `[Review brief](docs/plan.md)` or `[x](src/a.ts:42)`; the browser
// would resolve such an href against the page origin and navigate, which
// Sulion neither serves nor survives. Those open a file tab through the
// app command layer instead. Only absolute URLs reach the browser, in a
// new tab so the app keeps its state.

import type { Maybe } from "../../lib/types";

export interface FileLinkTarget {
  repo: string;
  workspaceId?: string;
  rootPath?: string;
}

export type MarkdownLink =
  | { kind: "external"; href: string }
  | { kind: "file"; path: string; line?: number }
  | { kind: "inert" };

const ABSOLUTE_URL = /^[a-z][a-z0-9+.-]*:/i;
const TRAILING_LINE = /:(\d{1,7})(?::\d{1,7})?$/;
const HASH_LINE = /#L(\d{1,7})(?:-L?\d{1,7})?$/;

/** Decide what a markdown href should do. `repo` is the repo of the
 * timeline the text belongs to, used to trim an absolute checkout path
 * (`/home/u/repos/<repo>/src/a.ts`) down to a repo-relative one. */
export function classifyMarkdownLink(
  href: Maybe<string>,
  repo: Maybe<string | null>,
  rootPath?: string,
): MarkdownLink {
  if (!href) return { kind: "inert" };
  const trimmed = href.trim();
  if (trimmed === "" || trimmed.startsWith("#")) return { kind: "inert" };

  if (ABSOLUTE_URL.test(trimmed)) {
    if (/^https?:|^mailto:/i.test(trimmed)) return { kind: "external", href: trimmed };
    if (/^file:\/\//i.test(trimmed)) {
      return fileLink(safeDecode(trimmed.replace(/^file:\/\//i, "")), repo, rootPath);
    }
    // javascript:, data:, and friends never reach the browser.
    return { kind: "inert" };
  }

  return fileLink(safeDecode(trimmed), repo, rootPath);
}

function fileLink(raw: string, repo: Maybe<string | null>, rootPath?: string): MarkdownLink {
  if (!repo) return { kind: "inert" };
  let path = raw;
  let line: Maybe<number>;

  const hash = HASH_LINE.exec(path);
  if (hash) {
    line = Number(hash[1]);
    path = path.slice(0, hash.index);
  } else {
    const hashAt = path.indexOf("#");
    if (hashAt !== -1) path = path.slice(0, hashAt);
  }
  const queryAt = path.indexOf("?");
  if (queryAt !== -1) path = path.slice(0, queryAt);

  const colon = TRAILING_LINE.exec(path);
  if (colon) {
    line = Number(colon[1]);
    path = path.slice(0, colon.index);
  }

  path = relativeToRepo(path, repo, rootPath);
  if (path === "" || path.split("/").includes("..")) return { kind: "inert" };
  return line == null ? { kind: "file", path } : { kind: "file", path, line };
}

/** Strip `./` prefixes and, for an absolute checkout path, everything up
 * to and including the `/<repo>/` segment. Workspace roots are supplied
 * explicitly because their directory includes a workspace ID. Absolute paths
 * outside the target checkout stay inert. */
function relativeToRepo(path: string, repo: string, rootPath?: string): string {
  let out = path.replace(/^(?:\.\/)+/, "");
  if (out.startsWith("/")) {
    const root = rootPath?.replace(/\/+$/, "");
    if (root && out.startsWith(`${root}/`)) return out.slice(root.length + 1).replace(/\/+$/, "");
    const marker = `/${repo}/`;
    const at = out.indexOf(marker);
    if (at === -1) return "";
    out = out.slice(at + marker.length);
  }
  return out.replace(/\/+$/, "");
}

function safeDecode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}
