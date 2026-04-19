// Compact one-line row for the timeline list (ticket #28). Selection
// drives the inspector pane; this row itself never expands inline.

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
      <span className="tr__prompt">{preview}</span>
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
            💭 {turn.thinkingCount}
          </span>
        )}
        {turn.hasErrors && (
          <span className="tr__badge tr__badge--error" title="errors in turn">
            ⚠
          </span>
        )}
      </span>
      <span className="tr__meta">
        {formatDuration(turn.durationMs)} · {formatTime(turn.startTimestamp)}
      </span>
    </button>
  );
}

function turnPreview(turn: Turn): string {
  if (turn.userPrompt) {
    const txt = userPromptText(turn.userPrompt);
    if (txt) return txt.replace(/\s+/g, " ").slice(0, 240);
  }
  const firstAssistant = turn.events.find((e) => e.kind === "assistant");
  if (firstAssistant) {
    const txt = textBlocksIn(firstAssistant).join(" ");
    if (txt) return `(assistant) ${txt.slice(0, 220)}`;
  }
  return "(no user prompt)";
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
      label: n === 1 ? name : `${name} ×${n}`,
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
  return `${m}m${rs > 0 ? ` ${rs}s` : ""}`;
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}
