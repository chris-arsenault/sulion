// Compact one-item row for the timeline list (ticket #28). Two-line
// layout so it reads cleanly in a narrow column: the prompt preview
// takes the top line, a smaller meta row underneath carries tool
// badges + duration. Selection drives the inspector pane; this row
// itself never expands inline.

import type { Turn, ToolPair } from "./grouping";
import { userPromptText, textBlocksIn } from "./types";
import "./TurnRow.css";

interface Props {
  turn: Turn;
  selected: boolean;
  showThinking: boolean;
  onSelect: () => void;
}

export function TurnRow({ turn, selected, showThinking, onSelect }: Props) {
  const preview = turnPreview(turn);
  const badges = toolBadges(turn.toolPairs);

  return (
    <button
      type="button"
      className={`tr ${selected ? "tr--selected" : ""} ${
        turn.hasErrors ? "tr--errors" : ""
      }`}
      onClick={onSelect}
      data-testid="turn-row"
      aria-pressed={selected}
    >
      <div className="tr__prompt">{preview}</div>
      <div className="tr__meta">
        <span className="tr__time">{formatTime(turn.startTimestamp)}</span>
        {turn.durationMs > 0 && (
          <span className="tr__dot" aria-hidden>
            ·
          </span>
        )}
        {turn.durationMs > 0 && (
          <span className="tr__duration">{formatDuration(turn.durationMs)}</span>
        )}
        {badges.length > 0 && (
          <span className="tr__dot" aria-hidden>
            ·
          </span>
        )}
        <span className="tr__badges">
          {badges.map((b) => (
            <span
              key={b.label}
              className={`tr__badge tr__badge--${b.variant}`}
              title={b.title}
            >
              {b.label}
            </span>
          ))}
          {turn.thinkingCount > 0 && showThinking && (
            <span className="tr__badge tr__badge--thinking" title="thinking">
              💭{turn.thinkingCount}
            </span>
          )}
          {turn.hasErrors && (
            <span className="tr__badge tr__badge--error" title="errors in turn">
              ⚠
            </span>
          )}
        </span>
      </div>
    </button>
  );
}

function turnPreview(turn: Turn): string {
  if (turn.userPrompt) {
    const txt = userPromptText(turn.userPrompt);
    if (txt) return firstParagraph(txt, 280);
  }
  const firstAssistant = turn.events.find((e) => e.kind === "assistant");
  if (firstAssistant) {
    const txt = textBlocksIn(firstAssistant).join(" ");
    if (txt) return `(assistant) ${firstParagraph(txt, 260)}`;
  }
  return "(no user prompt)";
}

/** Prompts are often multi-paragraph. In the list row we only have two
 * lines of height to work with, so:
 *   - split on the first blank line (paragraph break)
 *   - collapse internal whitespace inside that paragraph so CSS wrap
 *     handles the visual layout
 *   - if there are more paragraphs below, add a trailing " …" to hint
 *     at more content — the full prompt lives in the inspector pane
 *   - truncate if the paragraph alone exceeds `max` */
function firstParagraph(s: string, max: number): string {
  const trimmed = s.trim();
  if (!trimmed) return "";
  const paragraphs = trimmed.split(/\n\s*\n/);
  const first = (paragraphs[0] ?? trimmed).replace(/\s+/g, " ").trim();
  const hasMore = paragraphs.length > 1 && paragraphs.slice(1).some((p) => p.trim().length > 0);
  if (first.length <= max) {
    return hasMore ? `${first} …` : first;
  }
  return `${first.slice(0, max - 1)}…`;
}

interface Badge {
  label: string;
  variant: string;
  title: string;
}

function toolBadges(pairs: ToolPair[]): Badge[] {
  if (pairs.length === 0) return [];
  const counts = new Map<string, number>();
  for (const p of pairs) counts.set(p.name, (counts.get(p.name) ?? 0) + 1);
  return Array.from(counts.entries())
    .sort((a, b) => b[1] - a[1])
    .map(([name, n]) => ({
      label: n === 1 ? name : `${name}×${n}`,
      variant: name.toLowerCase(),
      title: `${n} ${name} call${n === 1 ? "" : "s"}`,
    }));
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const rs = Math.round(s - m * 60);
  return `${m}m${rs > 0 ? `${rs}s` : ""}`;
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
}
