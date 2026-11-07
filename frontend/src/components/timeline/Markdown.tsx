// Thin wrapper around react-markdown with our dark-theme overrides,
// GitHub-flavoured markdown (tables, strikethrough, task lists) and TeX
// math rendered through KaTeX. Used for user prompts and assistant text in
// TurnDetail.

import { memo, useCallback, useMemo, type MouseEvent, type ReactNode } from "react";
import ReactMarkdown, { type Options } from "react-markdown";
import rehypeKatex from "rehype-katex";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";

import "katex/dist/katex.min.css";
import "./Markdown.css";
import { appCommands } from "../../state/AppCommands";
import { classifyMarkdownLink, type FileLinkTarget } from "./markdownLinks";
import { normalizeMathDelimiters } from "./mathDelimiters";
import { remarkFileReferences } from "./remarkFileReferences";
import { AuthenticatedImage } from "../common/AuthenticatedImage";
import { Tooltip } from "../ui";

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
  /** Repo (and workspace) that relative links resolve against. Without
   * it a relative link renders as plain text, never as navigation. */
  fileTarget?: FileLinkTarget | null;
}

export const Markdown = memo(function Markdown({ source, compact = false, fileTarget = null }: Props) {
  const normalized = useMemo(() => normalizeMathDelimiters(source), [source]);
  const remarkPlugins = useMemo<NonNullable<Options["remarkPlugins"]>>(
    () => [...REMARK_PLUGINS, [remarkFileReferences, fileTarget]],
    [fileTarget],
  );
  const components = useMemo<NonNullable<Options["components"]>>(
    () => ({
      a: ({ href, children }: { href?: string; children?: ReactNode }) => (
        <MarkdownAnchor href={href} fileTarget={fileTarget}>
          {children}
        </MarkdownAnchor>
      ),
      img: ({ src, alt }) => <MarkdownImage src={src} alt={alt ?? ""} fileTarget={fileTarget} />,
    }),
    [fileTarget],
  );
  return (
    <div className={`md ${compact ? "md--compact" : ""}`}>
      <ReactMarkdown
        remarkPlugins={remarkPlugins}
        rehypePlugins={REHYPE_PLUGINS}
        components={components}
        // react-markdown v9 only allows safe HTML by default; no
        // rehype-raw means raw HTML in markdown is rendered as text,
        // which is what we want for user-supplied content.
      >
        {normalized}
      </ReactMarkdown>
    </div>
  );
});

/** Links never navigate this document. Absolute URLs open a new browser
 * tab; repo-relative paths open a Sulion file tab through the app
 * command layer; anything else is rendered as text. */
function MarkdownAnchor({
  href,
  fileTarget,
  children,
}: {
  href?: string;
  fileTarget: FileLinkTarget | null;
  children?: ReactNode;
}) {
  const link = useMemo(
    () => classifyMarkdownLink(href, fileTarget?.repo, fileTarget?.rootPath),
    [href, fileTarget?.repo, fileTarget?.rootPath],
  );
  const openFile = useCallback(
    (event: MouseEvent<HTMLAnchorElement>) => {
      event.preventDefault();
      if (link.kind !== "file" || !fileTarget) return;
      appCommands.openFile({
        repo: fileTarget.repo,
        workspaceId: fileTarget.workspaceId,
        path: link.path,
        line: link.line,
      });
    },
    [fileTarget, link],
  );

  if (link.kind === "external") {
    return (
      <a href={link.href} target="_blank" rel="noopener noreferrer">
        {children}
      </a>
    );
  }
  if (link.kind === "file") {
    const label = link.line == null ? link.path : `${link.path}:${link.line}`;
    return (
      <Tooltip label={`Open ${label}`}>
        <a href={href} className="md__file-link" onClick={openFile}>
          {children}
        </a>
      </Tooltip>
    );
  }
  return <span className="md__dead-link">{children}</span>;
}

function MarkdownImage({ src, alt, fileTarget }: {
  src?: string;
  alt: string;
  fileTarget: FileLinkTarget | null;
}) {
  const image = classifyMarkdownLink(src, fileTarget?.repo, fileTarget?.rootPath);
  if (image.kind === "file" && fileTarget) {
    return <AuthenticatedImage repo={fileTarget.repo} workspaceId={fileTarget.workspaceId}
      path={image.path} alt={alt || image.path} className="md__image" />;
  }
  if (image.kind === "external" && /^https?:/i.test(image.href)) {
    return <img src={image.href} alt={alt} className="md__image" loading="lazy" />;
  }
  return <span className="md__dead-link">{alt}</span>;
}
