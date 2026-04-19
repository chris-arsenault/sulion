// Full detail view of a turn. Rendered inside the inspector pane (or
// an overlay modal on narrow viewports). Replaces the inline
// expansion the old TurnBlock did — turns are now peek-via-row,
// drill-in-via-inspector.
//
// Thinking content is accessed via 💭 chips that open ThinkingFlyout
// (#29). Tool-use rows surface a peek card on hover via ToolHoverCard
// (#31). The header is position: sticky so it stays pinned while the
// user scrolls a long turn's detail (#30).

import { type MouseEvent, useRef, useState } from "react";

import type { TimelineEvent } from "../../api/types";
import {
  eventIsVisible,
  toolPairIsVisible,
  type TimelineFilters,
} from "./filters";
import { type ToolPair, type Turn } from "./grouping";
import { ThinkingFlyout } from "./ThinkingFlyout";
import { ToolHoverCard } from "./ToolHoverCard";
import { ToolCallRenderer } from "./tools/renderers";
import {
  flattenContent,
  payloadOf,
  textBlocksIn,
  thinkingBlocksIn,
  userPromptText,
} from "./types";
import "./TurnDetail.css";

interface Props {
  turn: Turn;
  showThinking: boolean;
  onOpenSubagent?: (pair: ToolPair) => void;
  /** Optional: when provided, events from hidden speakers and tool
   * pairs with hidden tool names are skipped. When absent, everything
   * renders (used by SubagentModal where there's no filter UI). */
  filters?: TimelineFilters;
}

interface ThinkingAnchor {
  el: HTMLElement;
  text: string;
}

interface HoverAnchor {
  el: HTMLElement;
  pair: ToolPair;
  pinned: boolean;
}

export function TurnDetail({ turn, showThinking, onOpenSubagent, filters }: Props) {
  const pairById = new Map(turn.toolPairs.map((p) => [p.id, p] as const));
  const [thinking, setThinking] = useState<ThinkingAnchor | null>(null);
  const [hover, setHover] = useState<HoverAnchor | null>(null);
  const dismissTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const openHover = (el: HTMLElement, pair: ToolPair) => {
    if (dismissTimer.current) clearTimeout(dismissTimer.current);
    setHover((prev) => {
      if (prev?.pinned && prev.pair.id === pair.id) return prev;
      return { el, pair, pinned: prev?.pinned && prev.pair.id === pair.id ? true : false };
    });
  };
  const scheduleDismiss = () => {
    if (dismissTimer.current) clearTimeout(dismissTimer.current);
    dismissTimer.current = setTimeout(() => {
      setHover((prev) => (prev?.pinned ? prev : null));
    }, 180);
  };

  return (
    <div className="td">
      <div className="td__header">
        <div className="td__header-prompt">
          <span className="td__header-label">Prompt</span>
          <pre className="td__prompt-text">
            {turn.userPrompt
              ? userPromptText(turn.userPrompt) || "(no prompt text)"
              : "(orphan turn — no user prompt)"}
          </pre>
        </div>
        <div className="td__header-meta">
          <span>{turn.events.length} events</span>
          <span>{turn.toolPairs.length} tool calls</span>
          {turn.thinkingCount > 0 && showThinking && (
            <span>💭 {turn.thinkingCount}</span>
          )}
          {turn.hasErrors && <span className="td__errors">⚠ errors</span>}
        </div>
      </div>

      <div className="td__body" data-testid="turn-detail">
        {turn.events.map((ev) => {
          if (ev.kind === "user" && ev === turn.userPrompt) return null;
          if (ev.kind === "user") return null; // tool_result wrapper, surfaced via pairs
          if (filters && !eventIsVisible(ev, filters)) return null;
          if (ev.kind === "assistant") {
            return (
              <AssistantRow
                key={ev.byte_offset}
                event={ev}
                pairById={pairById}
                showThinking={showThinking}
                onOpenSubagent={onOpenSubagent}
                filters={filters}
                onThinkingChip={(el, text) => {
                  setHover(null);
                  setThinking({ el, text });
                }}
                onToolEnter={openHover}
                onToolLeave={scheduleDismiss}
              />
            );
          }
          if (ev.kind === "summary") {
            return (
              <div key={ev.byte_offset} className="td__sub td__sub--summary">
                <span className="td__sub-label">summary</span>
                <span>
                  {flattenContent(payloadOf(ev).message?.content) || summaryTextOf(ev)}
                </span>
              </div>
            );
          }
          if (ev.kind === "system") {
            return (
              <div key={ev.byte_offset} className="td__sub td__sub--system">
                <span className="td__sub-label">system</span>
                <span>
                  {payloadOf(ev).subtype ?? "system"}{" "}
                  {flattenContent(payloadOf(ev).message?.content)}
                </span>
              </div>
            );
          }
          return (
            <div key={ev.byte_offset} className="td__sub td__sub--generic">
              <span className="td__sub-label">{ev.kind}</span>
              <details>
                <summary>raw</summary>
                <pre>{JSON.stringify(ev.payload, null, 2)}</pre>
              </details>
            </div>
          );
        })}
      </div>

      {thinking && showThinking && (
        <ThinkingFlyout
          anchor={thinking.el}
          thinkingText={thinking.text}
          onClose={() => setThinking(null)}
        />
      )}
      {hover && (
        <div
          onMouseEnter={() => {
            if (dismissTimer.current) clearTimeout(dismissTimer.current);
          }}
          onMouseLeave={scheduleDismiss}
        >
          <ToolHoverCard
            anchor={hover.el}
            pair={hover.pair}
            pinned={hover.pinned}
            onPin={() =>
              setHover((prev) => (prev ? { ...prev, pinned: true } : prev))
            }
            onClose={() => setHover(null)}
          />
        </div>
      )}
    </div>
  );
}

function AssistantRow({
  event,
  pairById,
  showThinking,
  onOpenSubagent,
  filters,
  onThinkingChip,
  onToolEnter,
  onToolLeave,
}: {
  event: TimelineEvent;
  pairById: Map<string, ToolPair>;
  showThinking: boolean;
  onOpenSubagent?: (pair: ToolPair) => void;
  filters?: TimelineFilters;
  onThinkingChip: (el: HTMLElement, text: string) => void;
  onToolEnter: (el: HTMLElement, pair: ToolPair) => void;
  onToolLeave: () => void;
}) {
  const texts = textBlocksIn(event);
  const thoughts = thinkingBlocksIn(event);
  const toolUseIds =
    (payloadOf(event).message?.content as { type: string; id?: string }[] | undefined)
      ?.filter((b) => b.type === "tool_use")
      .map((b) => b.id ?? "") ?? [];

  return (
    <div className="td__sub td__sub--assistant">
      {texts.map((t, i) => (
        <p key={`t-${i}`} className="td__text">
          {t}
        </p>
      ))}
      {showThinking && thoughts.length > 0 && (
        <div className="td__thinking-chips">
          {thoughts.map((th, i) => (
            <button
              key={`k-${i}`}
              type="button"
              className="td__thinking-chip"
              onClick={(e: MouseEvent<HTMLButtonElement>) =>
                onThinkingChip(e.currentTarget, th.thinking ?? "")
              }
              title="View thinking"
            >
              💭 thinking{thoughts.length > 1 ? ` ${i + 1}/${thoughts.length}` : ""}
            </button>
          ))}
        </div>
      )}
      {toolUseIds.map((id) => {
        const pair = pairById.get(id);
        if (!pair) return null;
        if (filters && !toolPairIsVisible(pair, filters)) return null;
        return (
          <ToolPairRow
            key={pair.id || id}
            pair={pair}
            onOpenSubagent={onOpenSubagent}
            onEnter={(el) => onToolEnter(el, pair)}
            onLeave={onToolLeave}
          />
        );
      })}
    </div>
  );
}

function ToolPairRow({
  pair,
  onOpenSubagent,
  onEnter,
  onLeave,
}: {
  pair: ToolPair;
  onOpenSubagent?: (pair: ToolPair) => void;
  onEnter: (el: HTMLElement) => void;
  onLeave: () => void;
}) {
  // Low-signal rendering: successful non-pending pair collapses to a
  // single line. Errors/pending expand automatically.
  const lowSignal = !pair.isError && !pair.isPending;
  const [expanded, setExpanded] = useState(!lowSignal);
  const rowRef = useRef<HTMLDivElement>(null);
  const enterTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const handleEnter = () => {
    if (enterTimer.current) clearTimeout(enterTimer.current);
    enterTimer.current = setTimeout(() => {
      if (rowRef.current) onEnter(rowRef.current);
    }, 160);
  };
  const handleLeave = () => {
    if (enterTimer.current) {
      clearTimeout(enterTimer.current);
      enterTimer.current = null;
    }
    onLeave();
  };

  return (
    <div
      ref={rowRef}
      className={`td__tool ${pair.isError ? "td__tool--error" : ""} ${
        pair.isPending ? "td__tool--pending" : ""
      }`}
      onMouseEnter={handleEnter}
      onMouseLeave={handleLeave}
      data-testid="tool-pair-row"
    >
      <div className="td__tool-header">
        <button
          type="button"
          className="td__tool-toggle"
          onClick={() => setExpanded((v) => !v)}
          aria-label={expanded ? "Collapse tool details" : "Expand tool details"}
        >
          <span className="td__tool-chevron">{expanded ? "▾" : "▸"}</span>
          <span className={`td__tool-name td__tool-name--${pair.name.toLowerCase()}`}>
            {pair.name}
          </span>
          <span className="td__tool-summary">{toolSummary(pair)}</span>
          {pair.isPending && <span className="td__tool-status">pending</span>}
          {pair.isError && (
            <span className="td__tool-status td__tool-status--error">error</span>
          )}
          {!expanded && !pair.isError && !pair.isPending && (
            <span className="td__tool-status td__tool-status--ok">ok</span>
          )}
        </button>
        {pair.name === "Task" && onOpenSubagent && (
          <button
            type="button"
            className="td__tool-subagent"
            onClick={(e) => {
              e.stopPropagation();
              onOpenSubagent(pair);
            }}
          >
            View agent log →
          </button>
        )}
      </div>
      {expanded && (
        <div className="td__tool-body">
          <ToolCallRenderer
            tool={{ id: pair.id, name: pair.name, input: pair.input }}
          />
          {pair.result && <ToolResultRender pair={pair} />}
        </div>
      )}
    </div>
  );
}

function ToolResultRender({ pair }: { pair: ToolPair }) {
  const r = pair.result!;
  const body =
    typeof r.content === "string"
      ? r.content
      : flattenContent(r.content);
  const truncated =
    body.length > 2000 ? `${body.slice(0, 2000)}\n… (${body.length} chars)` : body;
  return (
    <div className={`td__tool-result ${r.is_error ? "td__tool-result--error" : ""}`}>
      <div className="td__tool-result-label">
        result{r.is_error ? " (error)" : ""}
      </div>
      <pre>{truncated || "(empty result)"}</pre>
    </div>
  );
}

function toolSummary(pair: ToolPair): string {
  const input = (pair.input ?? {}) as Record<string, unknown>;
  const pick = (k: string) =>
    typeof input[k] === "string" ? (input[k] as string) : undefined;
  switch (pair.name) {
    case "Edit":
    case "Write":
    case "MultiEdit":
    case "Read":
      return pick("file_path") ?? "";
    case "Bash":
      return (pick("command") ?? "").slice(0, 120);
    case "Grep":
      return pick("pattern") ?? "";
    case "Glob":
      return pick("pattern") ?? "";
    case "Task":
      return pick("description") ?? pick("subagent_type") ?? "";
    case "TodoWrite":
      return "todos updated";
    case "WebFetch":
      return pick("url") ?? "";
    case "WebSearch":
      return pick("query") ?? "";
    default:
      return "";
  }
}

function summaryTextOf(ev: TimelineEvent): string {
  const p = payloadOf(ev);
  const s = (p as { summary?: unknown }).summary;
  return typeof s === "string" ? s : "";
}
