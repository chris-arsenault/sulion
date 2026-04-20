import { useCallback, useMemo } from "react";

import type { ToolPair, Turn } from "./grouping";
import { Icon } from "../../icons";
import { Tooltip } from "../ui";
import "./TurnRow.css";

interface Props {
  turn: Turn;
  selected: boolean;
  showThinking: boolean;
  onSelect: (id: number) => void;
}

export function TurnRow({ turn, selected, showThinking, onSelect }: Props) {
  const badges = useMemo(() => toolBadges(turn.tool_pairs), [turn.tool_pairs]);
  const onClick = useCallback(() => onSelect(turn.id), [onSelect, turn.id]);

  return (
    <button
      type="button"
      className={`tr ${selected ? "tr--selected" : ""} ${
        turn.has_errors ? "tr--errors" : ""
      }`}
      onClick={onClick}
      data-testid="turn-row"
      aria-pressed={selected}
    >
      <div className="tr__prompt">{turn.preview}</div>
      <div className="tr__meta">
        <span className="tr__time tabular">{formatTime(turn.start_timestamp)}</span>
        {turn.duration_ms > 0 && (
          <span className="tr__dot" aria-hidden>
            ·
          </span>
        )}
        {turn.duration_ms > 0 && (
          <span className="tr__duration tabular">
            {formatDuration(turn.duration_ms)}
          </span>
        )}
        {badges.length > 0 && (
          <span className="tr__dot" aria-hidden>
            ·
          </span>
        )}
        <span className="tr__badges">
          {badges.map((badge) => (
            <Tooltip key={badge.label} label={badge.title}>
              <span className={`tr__badge tr__badge--${badge.variant}`}>
                {badge.label}
              </span>
            </Tooltip>
          ))}
          {turn.thinking_count > 0 && showThinking && (
            <Tooltip label="Thinking blocks in this turn">
              <span className="tr__badge tr__badge--thinking">
                <Icon name="sparkles" size={12} />
                <span className="tabular">{turn.thinking_count}</span>
              </span>
            </Tooltip>
          )}
          {turn.has_errors && (
            <Tooltip label="Errors in this turn">
              <span className="tr__badge tr__badge--error">
                <Icon name="alert-triangle" size={12} />
              </span>
            </Tooltip>
          )}
        </span>
      </div>
    </button>
  );
}

interface Badge {
  label: string;
  variant: string;
  title: string;
}

function toolBadges(pairs: ToolPair[]): Badge[] {
  if (pairs.length === 0) return [];
  const counts = new Map<string, number>();
  for (const pair of pairs) {
    const name = pair.operation_type ?? pair.name;
    counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  return Array.from(counts.entries())
    .sort((a, b) => b[1] - a[1])
    .map(([name, count]) => ({
      label: count === 1 ? name : `${name}×${count}`,
      variant: name.toLowerCase(),
      title: `${count} ${name} call${count === 1 ? "" : "s"}`,
    }));
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const minutes = Math.floor(seconds / 60);
  const remainder = Math.round(seconds - minutes * 60);
  const seconds_suffix = remainder > 0 ? `${remainder}s` : "";
  return `${minutes}m${seconds_suffix}`;
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
}
