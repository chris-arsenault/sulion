export interface InlineDiffPart {
  kind: "same" | "added" | "removed";
  value: string;
}

export type InlineCodeDiffRow =
  | {
      kind: "context";
      oldLine: number;
      newLine: number;
      parts: InlineDiffPart[];
    }
  | {
      kind: "removed";
      oldLine: number;
      newLine: null;
      parts: InlineDiffPart[];
    }
  | {
      kind: "added";
      oldLine: null;
      newLine: number;
      parts: InlineDiffPart[];
    }
  | {
      kind: "collapsed";
      count: number;
    };

export interface InlineCodeDiffModel {
  state: "changed" | "whitespace_only" | "unchanged";
  rows: InlineCodeDiffRow[];
}

interface SequenceOp {
  kind: "context" | "removed" | "added";
  text: string;
}

const TOKEN_RE = /(\s+|[A-Za-z0-9_]+|[^A-Za-z0-9_\s])/g;

export function buildInlineCodeDiff(
  oldText: string,
  newText: string,
  options: { compact?: boolean } = {},
): InlineCodeDiffModel {
  if (oldText === newText) {
    return { state: "unchanged", rows: [] };
  }

  const oldLines = toDisplayLines(oldText);
  const newLines = toDisplayLines(newText);
  const ops = diffSequence(oldLines, newLines, normalizeLineForMatch);
  const hasMaterialChanges = ops.some((op) => op.kind !== "context");
  if (!hasMaterialChanges) {
    return { state: "whitespace_only", rows: [] };
  }

  const rows = buildRows(ops);
  return {
    state: "changed",
    rows: collapseContextRows(rows, options.compact ? 1 : 2),
  };
}

function buildRows(ops: SequenceOp[]): InlineCodeDiffRow[] {
  const rows: InlineCodeDiffRow[] = [];
  let oldLine = 1;
  let newLine = 1;
  let idx = 0;

  while (idx < ops.length) {
    const current = ops[idx];
    if (!current) {
      break;
    }
    if (current.kind === "context") {
      rows.push({
        kind: "context",
        oldLine,
        newLine,
        parts: [{ kind: "same", value: current.text }],
      });
      oldLine += 1;
      newLine += 1;
      idx += 1;
      continue;
    }

    const removed: string[] = [];
    const added: string[] = [];
    while (ops[idx]?.kind === "removed") {
      removed.push(ops[idx]!.text);
      idx += 1;
    }
    while (ops[idx]?.kind === "added") {
      added.push(ops[idx]!.text);
      idx += 1;
    }

    if (removed.length === added.length && removed.length > 0) {
      for (let pairIdx = 0; pairIdx < removed.length; pairIdx += 1) {
        const oldText = removed[pairIdx] ?? "";
        const newText = added[pairIdx] ?? "";
        const paired = diffInlineParts(oldText, newText);
        rows.push({
          kind: "removed",
          oldLine,
          newLine: null,
          parts: paired.oldParts,
        });
        rows.push({
          kind: "added",
          oldLine: null,
          newLine,
          parts: paired.newParts,
        });
        oldLine += 1;
        newLine += 1;
      }
      continue;
    }

    for (const text of removed) {
      rows.push({
        kind: "removed",
        oldLine,
        newLine: null,
        parts: [{ kind: "removed", value: text }],
      });
      oldLine += 1;
    }
    for (const text of added) {
      rows.push({
        kind: "added",
        oldLine: null,
        newLine,
        parts: [{ kind: "added", value: text }],
      });
      newLine += 1;
    }
  }

  return rows;
}

function collapseContextRows(rows: InlineCodeDiffRow[], contextRadius: number) {
  const changedIndexes = rows
    .map((row, idx) => (row.kind === "context" || row.kind === "collapsed" ? null : idx))
    .filter((idx): idx is number => idx !== null);

  if (changedIndexes.length === 0) {
    return rows;
  }

  const keep = new Set<number>();
  for (const idx of changedIndexes) {
    for (
      let cursor = Math.max(0, idx - contextRadius);
      cursor <= Math.min(rows.length - 1, idx + contextRadius);
      cursor += 1
    ) {
      if (rows[cursor]?.kind === "context") {
        keep.add(cursor);
      }
    }
    keep.add(idx);
  }

  const out: InlineCodeDiffRow[] = [];
  let hiddenCount = 0;
  const flushHidden = () => {
    if (hiddenCount > 0) {
      out.push({ kind: "collapsed", count: hiddenCount });
      hiddenCount = 0;
    }
  };

  for (let idx = 0; idx < rows.length; idx += 1) {
    const row = rows[idx];
    if (!row) {
      continue;
    }
    if (row.kind === "context" && !keep.has(idx)) {
      hiddenCount += 1;
      continue;
    }
    flushHidden();
    out.push(row);
  }
  flushHidden();
  return out;
}

function diffInlineParts(oldText: string, newText: string) {
  const oldTokens = tokenize(oldText);
  const newTokens = tokenize(newText);
  const ops = diffSequence(oldTokens, newTokens, (token) => token);
  const oldParts: InlineDiffPart[] = [];
  const newParts: InlineDiffPart[] = [];

  for (const op of ops) {
    if (op.kind === "context") {
      pushPart(oldParts, "same", op.text);
      pushPart(newParts, "same", op.text);
    } else if (op.kind === "removed") {
      pushPart(oldParts, "removed", op.text);
    } else {
      pushPart(newParts, "added", op.text);
    }
  }

  return { oldParts, newParts };
}

function diffSequence(
  oldItems: string[],
  newItems: string[],
  normalize: (item: string) => string,
): SequenceOp[] {
  const oldNorm = oldItems.map(normalize);
  const newNorm = newItems.map(normalize);
  const dp = Array.from({ length: oldNorm.length + 1 }, () =>
    Array(newNorm.length + 1).fill(0),
  );

  for (let oldIdx = oldNorm.length - 1; oldIdx >= 0; oldIdx -= 1) {
    for (let newIdx = newNorm.length - 1; newIdx >= 0; newIdx -= 1) {
      if (oldNorm[oldIdx] === newNorm[newIdx]) {
        dp[oldIdx]![newIdx] = (dp[oldIdx + 1]?.[newIdx + 1] ?? 0) + 1;
      } else {
        dp[oldIdx]![newIdx] = Math.max(
          dp[oldIdx + 1]?.[newIdx] ?? 0,
          dp[oldIdx]?.[newIdx + 1] ?? 0,
        );
      }
    }
  }

  const out: SequenceOp[] = [];
  let oldIdx = 0;
  let newIdx = 0;
  while (oldIdx < oldItems.length && newIdx < newItems.length) {
    if (oldNorm[oldIdx] === newNorm[newIdx]) {
      out.push({ kind: "context", text: oldItems[oldIdx] ?? "" });
      oldIdx += 1;
      newIdx += 1;
      continue;
    }
    if ((dp[oldIdx + 1]?.[newIdx] ?? 0) >= (dp[oldIdx]?.[newIdx + 1] ?? 0)) {
      out.push({ kind: "removed", text: oldItems[oldIdx] ?? "" });
      oldIdx += 1;
    } else {
      out.push({ kind: "added", text: newItems[newIdx] ?? "" });
      newIdx += 1;
    }
  }
  while (oldIdx < oldItems.length) {
    out.push({ kind: "removed", text: oldItems[oldIdx] ?? "" });
    oldIdx += 1;
  }
  while (newIdx < newItems.length) {
    out.push({ kind: "added", text: newItems[newIdx] ?? "" });
    newIdx += 1;
  }
  return out;
}

function toDisplayLines(text: string) {
  const normalized = text.replace(/\r\n/g, "\n");
  if (!normalized) {
    return [];
  }
  const lines = normalized.split("\n");
  if (lines.at(-1) === "") {
    lines.pop();
  }
  return lines;
}

function normalizeLineForMatch(line: string) {
  return line.replace(/\s+/g, "");
}

function tokenize(line: string) {
  return line.match(TOKEN_RE) ?? [line];
}

function pushPart(parts: InlineDiffPart[], kind: InlineDiffPart["kind"], value: string) {
  if (!value) {
    return;
  }
  const prev = parts[parts.length - 1];
  if (prev?.kind === kind) {
    prev.value += value;
    return;
  }
  parts.push({ kind, value });
}
