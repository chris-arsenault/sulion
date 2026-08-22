// Compact turn navigation: one small square per turn, hover for the
// prompt preview, click to select. Rendered inside TurnGridFlyout when
// the timeline's nav mode is "grid".

import { useCallback } from "react";

import type { TurnSummary } from "./grouping";
import { Tooltip } from "../ui";
import "./TurnGrid.css";

export function TurnGrid({
  turns,
  selectedTurnKey,
  onSelect,
}: {
  turns: TurnSummary[];
  selectedTurnKey: string | null;
  onSelect: (key: string) => void;
}) {
  return (
    <div className="tg" data-testid="turn-grid" aria-label="Turn overview grid">
      {turns.map((turn) => (
        <TurnCell
          key={turn.turn_key ?? `${turn.id}`}
          turn={turn}
          selected={selectedTurnKey === (turn.turn_key ?? `${turn.id}`)}
          onSelect={onSelect}
        />
      ))}
    </div>
  );
}

function TurnCell({
  turn,
  selected,
  onSelect,
}: {
  turn: TurnSummary;
  selected: boolean;
  onSelect: (key: string) => void;
}) {
  const turnKey = turn.turn_key ?? `${turn.id}`;
  const onClick = useCallback(() => onSelect(turnKey), [onSelect, turnKey]);
  const time = formatTime(turn.start_timestamp);
  const label = `${time} · ${truncate(turn.preview, 120)}`;
  const cls = [
    "tg__cell",
    selected ? "tg__cell--selected" : "",
    turn.has_errors ? "tg__cell--errors" : "",
    turn.is_sidechain ? "tg__cell--sidechain" : "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <Tooltip label={label}>
      <button
        type="button"
        className={cls}
        onClick={onClick}
        aria-label={label}
        aria-pressed={selected}
      />
    </Tooltip>
  );
}

function truncate(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max)}…` : text;
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
