// Decides how a fetched file is presented. Kept out of FileTab.tsx so the
// dispatch can be tested directly: it is the point that decides whether file
// bytes reach the DOM as markup, and repo files are agent-authored.

import type { FileResponse } from "../api/types";

export type RenderKind =
  | { kind: "truncated" }
  | { kind: "image-binary" }
  | { kind: "binary" }
  | { kind: "markdown"; source: string }
  | { kind: "json"; value: unknown; parseError?: string }
  | { kind: "ndjson"; entries: Array<{ line: number; value: unknown; parseError?: string }> }
  | { kind: "code"; lang: string; code: string }
  | { kind: "raw"; code: string };

export function chooseRenderKind(data: FileResponse, raw: boolean): RenderKind {
  if (data.truncated) return { kind: "truncated" };
  if (data.binary) {
    if (data.mime.startsWith("image/")) {
      return {
        kind: "image-binary",
      };
    }
    return { kind: "binary" };
  }
  if (raw) return { kind: "raw", code: data.content ?? "" };
  // Text-sniffed SVG. Rendered as an image rather than inlined: the bytes come
  // from a repo an agent can write to, and an inlined <svg> executes its own
  // event handlers. Inside <img> it cannot script. The raw toggle above still
  // shows the source as text.
  if (data.mime === "image/svg+xml" && data.content) {
    return { kind: "image-binary" };
  }

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

export function canToggleRaw(data: FileResponse): boolean {
  if (data.truncated) return false;
  if (data.binary) return false;
  return true;
}

export function extensionOf(path: string): string {
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
