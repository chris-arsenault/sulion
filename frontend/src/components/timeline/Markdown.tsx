// Thin wrapper around react-markdown with our dark-theme overrides and
// GitHub-flavoured markdown (tables, strikethrough, task lists). Used
// for user prompts and assistant text in TurnDetail.

import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import "./Markdown.css";

const REMARK_PLUGINS = [remarkGfm];

interface Props {
  source: string;
  /** Compact variant tightens margins — used inside inline bubbles. */
  compact?: boolean;
}

export function Markdown({ source, compact = false }: Props) {
  return (
    <div className={`md ${compact ? "md--compact" : ""}`}>
      <ReactMarkdown
        remarkPlugins={REMARK_PLUGINS}
        // react-markdown v9 only allows safe HTML by default; no
        // rehype-raw means raw HTML in markdown is rendered as text,
        // which is what we want for user-supplied content.
      >
        {source}
      </ReactMarkdown>
    </div>
  );
}
