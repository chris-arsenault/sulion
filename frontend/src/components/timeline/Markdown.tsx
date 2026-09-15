// Thin wrapper around react-markdown with our dark-theme overrides,
// GitHub-flavoured markdown (tables, strikethrough, task lists) and TeX
// math rendered through KaTeX. Used for user prompts and assistant text in
// TurnDetail.

import { useMemo } from "react";
import ReactMarkdown, { type Options } from "react-markdown";
import rehypeKatex from "rehype-katex";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";

import "katex/dist/katex.min.css";
import "./Markdown.css";
import { normalizeMathDelimiters } from "./mathDelimiters";

// Single-dollar math is off: agent transcripts mention `$HOME`, `$1`, and
// prices far more often than they carry `$x$` formulas, and a stray pair
// would swallow the prose between them. Display math is `$$` on its own
// lines, inline math is `$$ … $$` within a line, and the LaTeX
// `\[ … \]` / `\( … \)` forms are rewritten to those first.
const REMARK_PLUGINS: NonNullable<Options["remarkPlugins"]> = [
  remarkGfm,
  [remarkMath, { singleDollarTextMath: false }],
];

// KaTeX's default `strict: "warn"` logs to the console for every
// non-strict construct; malformed math is still rendered as red source
// text (throwOnError is off inside rehype-katex), which is the right
// failure mode for content we do not author.
const REHYPE_PLUGINS: NonNullable<Options["rehypePlugins"]> = [
  [rehypeKatex, { strict: "ignore", errorColor: "var(--warn-fg)" }],
];

interface Props {
  source: string;
  /** Compact variant tightens margins — used inside inline bubbles. */
  compact?: boolean;
}

export function Markdown({ source, compact = false }: Props) {
  const normalized = useMemo(() => normalizeMathDelimiters(source), [source]);
  return (
    <div className={`md ${compact ? "md--compact" : ""}`}>
      <ReactMarkdown
        remarkPlugins={REMARK_PLUGINS}
        rehypePlugins={REHYPE_PLUGINS}
        // react-markdown v9 only allows safe HTML by default; no
        // rehype-raw means raw HTML in markdown is rendered as text,
        // which is what we want for user-supplied content.
      >
        {normalized}
      </ReactMarkdown>
    </div>
  );
}
