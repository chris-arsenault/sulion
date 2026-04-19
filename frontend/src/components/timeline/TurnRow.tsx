import { saveLibraryEntry } from "../../api/client";
import { appCommands } from "../../state/AppCommands";
import { formatTurn } from "./markdown-export";
import type { ToolPair, Turn } from "./grouping";
import type { MenuItem } from "../common/ContextMenu";
import {
  contextMenuHandler,
  useContextMenu,
} from "../common/ContextMenu";
import "./TurnRow.css";

interface Props {
  turn: Turn;
  selected: boolean;
  showThinking: boolean;
  onSelect: () => void;
  repo?: string;
  onError?: (message: string) => void;
}

export function TurnRow({
  turn,
  selected,
  showThinking,
  onSelect,
  repo,
  onError,
}: Props) {
  const badges = toolBadges(turn.tool_pairs);
  const openCtx = useContextMenu((store) => store.open);

  const onContextMenu = contextMenuHandler(openCtx, () => {
    const items: MenuItem[] = [];
    if (repo) {
      items.push({
        kind: "item",
        id: "pin-ref",
        label: "Pin turn as reference",
        onSelect: async () => {
          const name = defaultRefName(turn);
          try {
            await saveLibraryEntry(
              repo,
              "refs",
              { name, tags: ["turn"], body: formatTurn(turn) },
            );
            appCommands.libraryChanged({ repo, kind: "refs" });
          } catch (err) {
            onError?.(
              `Pin reference failed: ${err instanceof Error ? err.message : "save failed"}`,
            );
          }
        },
      });
    }
    return items;
  });

  return (
    <button
      type="button"
      className={`tr ${selected ? "tr--selected" : ""} ${
        turn.has_errors ? "tr--errors" : ""
      }`}
      onClick={onSelect}
      onContextMenu={repo ? onContextMenu : undefined}
      data-testid="turn-row"
      aria-pressed={selected}
    >
      <div className="tr__prompt">{turn.preview}</div>
      <div className="tr__meta">
        <span className="tr__time">{formatTime(turn.start_timestamp)}</span>
        {turn.duration_ms > 0 && (
          <span className="tr__dot" aria-hidden>
            ·
          </span>
        )}
        {turn.duration_ms > 0 && (
          <span className="tr__duration">{formatDuration(turn.duration_ms)}</span>
        )}
        {badges.length > 0 && (
          <span className="tr__dot" aria-hidden>
            ·
          </span>
        )}
        <span className="tr__badges">
          {badges.map((badge) => (
            <span
              key={badge.label}
              className={`tr__badge tr__badge--${badge.variant}`}
              title={badge.title}
            >
              {badge.label}
            </span>
          ))}
          {turn.thinking_count > 0 && showThinking && (
            <span className="tr__badge tr__badge--thinking" title="thinking">
              💭{turn.thinking_count}
            </span>
          )}
          {turn.has_errors && (
            <span className="tr__badge tr__badge--error" title="errors in turn">
              ⚠
            </span>
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
  return `${minutes}m${remainder > 0 ? `${remainder}s` : ""}`;
}

function defaultRefName(turn: Turn): string {
  if (turn.user_prompt_text) {
    const text = turn.user_prompt_text.replace(/\s+/g, " ").trim();
    if (text) return text.slice(0, 60);
  }
  const d = new Date(turn.start_timestamp);
  return Number.isFinite(d.getTime())
    ? `Turn at ${d.toLocaleString()}`
    : "Turn";
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
}
