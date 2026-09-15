// OpenAI-family models (Codex, ChatGPT) emit TeX math with the LaTeX
// delimiters `\[ … \]` (display) and `\( … \)` (inline). CommonMark treats
// the leading backslash as an escape, so react-markdown renders `\[` as a
// bare `[` and `\,` as `,` — the formula survives only as mangled text.
//
// remark-math understands the `$$` family. This pass rewrites the LaTeX
// delimiters into that family before parsing, outside fenced code and
// inline code spans so a shell snippet mentioning `\[` is left alone.
//
// Display math becomes a `$$` block on its own lines (remark-math parses
// double dollars on one line as text math, not flow math). Inline math
// becomes `$$ … $$` on one line because the Markdown component disables
// single-dollar math: agent transcripts are full of `$HOME`-style prose
// that would otherwise pair up into bogus formulas.

const FENCE = /^ {0,3}(`{3,}|~{3,})/;

export function normalizeMathDelimiters(source: string): string {
  if (!source.includes("\\[") && !source.includes("\\(")) return source;

  const lines = source.split("\n");
  const out: string[] = [];
  let fence: string | null = null;
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];
    if (fence) {
      out.push(line);
      if (closesFence(line, fence)) fence = null;
      i += 1;
      continue;
    }
    const open = FENCE.exec(line);
    if (open) {
      fence = open[1];
      out.push(line);
      i += 1;
      continue;
    }

    // Display math may span lines, so gather a paragraph (up to the next
    // blank line or fence) and rewrite it as one unit.
    let j = i;
    while (j < lines.length && lines[j].trim() !== "" && !FENCE.test(lines[j])) {
      j += 1;
    }
    if (j === i) {
      out.push(line);
      i += 1;
      continue;
    }
    out.push(rewriteParagraph(lines.slice(i, j).join("\n")));
    i = j;
  }

  return out.join("\n");
}

function closesFence(line: string, fence: string): boolean {
  const m = /^ {0,3}(`{3,}|~{3,})\s*$/.exec(line);
  return m !== null && m[1][0] === fence[0] && m[1].length >= fence.length;
}

function rewriteParagraph(text: string): string {
  let result = "";
  let pos = 0;
  while (pos < text.length) {
    const ch = text[pos];

    if (ch === "`") {
      const end = skipCodeSpan(text, pos);
      result += text.slice(pos, end);
      pos = end;
      continue;
    }

    if (ch === "\\" && pos + 1 < text.length) {
      const next = text[pos + 1];
      if (next === "[" || next === "(") {
        const close = next === "[" ? "\\]" : "\\)";
        const end = text.indexOf(close, pos + 2);
        if (end !== -1) {
          const body = text.slice(pos + 2, end).trim();
          result +=
            next === "["
              ? displayBlock(body, result, text.slice(end + 2))
              : `$$${body}$$`;
          pos = end + 2;
          continue;
        }
      }
      // Any other escape (including `\\`) passes through untouched.
      result += ch + next;
      pos += 2;
      continue;
    }

    result += ch;
    pos += 1;
  }
  return result;
}

/** Skip an inline code span starting at `pos` (a backtick run). Returns the
 * index just past its closing run, or past the opening run if unmatched. */
function skipCodeSpan(text: string, pos: number): number {
  let runEnd = pos;
  while (runEnd < text.length && text[runEnd] === "`") runEnd += 1;
  const run = text.slice(pos, runEnd);
  let search = runEnd;
  while (search < text.length) {
    const idx = text.indexOf(run, search);
    if (idx === -1) break;
    let candEnd = idx;
    while (candEnd < text.length && text[candEnd] === "`") candEnd += 1;
    if (candEnd - idx === run.length) return candEnd;
    search = candEnd;
  }
  return runEnd;
}

function displayBlock(body: string, before: string, after: string): string {
  const lead = before === "" || before.endsWith("\n") ? "" : "\n";
  const trail = after === "" || after.startsWith("\n") ? "" : "\n";
  return `${lead}\n$$\n${body}\n$$\n${trail}`;
}
