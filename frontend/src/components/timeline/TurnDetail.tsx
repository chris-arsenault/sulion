// Full detail view of a turn. Rendered inside the inspector pane (or
// an overlay modal on narrow viewports). Replaces the inline
// expansion the old TurnBlock did — turns are now peek-via-row,
// drill-in-via-inspector.
//
// Thinking content is accessed via 💭 chips that open ThinkingFlyout
// (#29). Tool-use rows surface a peek card on hover via ToolHoverCard
// (#31). The header is position: sticky so it stays pinned while the
// user scrolls a long turn's detail (#30).
//
// Assistant events are coalesced: consecutive assistant events with no
// intervening visible tool / summary / system event fold into a single
// rendered block with one set of copy actions. This keeps the detail
// readable when the user filters out Read/Edit/Write and the timeline
// collapses to mostly prose.

import { type MouseEvent, useMemo, useRef, useState } from "react";

import type { TimelineEvent } from "../../api/types";
import { CopyButton } from "./CopyButton";
import {
  eventIsVisible,
  toolPairIsVisible,
  type TimelineFilters,
} from "./filters";
import { type ToolPair, type Turn } from "./grouping";
import { Markdown } from "./Markdown";
import {
  formatAssistantEvent,
  formatAssistantText,
  formatTurn,
} from "./markdown-export";
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

type Chunk =
  | { kind: "assistant"; events: TimelineEvent[] }
  | { kind: "tool"; pair: ToolPair }
  | { kind: "summary"; event: TimelineEvent }
  | { kind: "system"; event: TimelineEvent }
  | { kind: "generic"; event: TimelineEvent };

export function TurnDetail({ turn, showThinking, onOpenSubagent, filters }: Props) {
  const pairById = useMemo(
    () => new Map(turn.toolPairs.map((p) => [p.id, p] as const)),
    [turn.toolPairs],
  );
  const chunks = useMemo(
    () => buildChunks(turn, pairById, filters),
    [turn, pairById, filters],
  );
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
          <div className="td__prompt-text">
            {turn.userPrompt ? (
              userPromptText(turn.userPrompt) ? (
                <Markdown source={userPromptText(turn.userPrompt)} />
              ) : (
                <span className="td__muted">(no prompt text)</span>
              )
            ) : (
              <span className="td__muted">(orphan turn — no user prompt)</span>
            )}
          </div>
        </div>
        <div className="td__header-meta">
          <span>{turn.events.length} events</span>
          <span>{turn.toolPairs.length} tool calls</span>
          {turn.thinkingCount > 0 && showThinking && (
            <span>💭 {turn.thinkingCount}</span>
          )}
          {turn.hasErrors && <span className="td__errors">⚠ errors</span>}
          <CopyButton
            getText={() => formatTurn(turn)}
            label="turn"
            icon="⧉"
            title="Copy this entire turn as markdown"
            className="td__copy-turn"
          />
        </div>
      </div>

      <div className="td__body" data-testid="turn-detail">
        {chunks.map((chunk, idx) => {
          if (chunk.kind === "assistant") {
            return (
              <AssistantBlock
                key={`a-${chunk.events[0]!.byte_offset}`}
                events={chunk.events}
                pairById={pairById}
                showThinking={showThinking}
                onThinkingChip={(el, text) => {
                  setHover(null);
                  setThinking({ el, text });
                }}
              />
            );
          }
          if (chunk.kind === "tool") {
            return (
              <ToolPairRow
                key={`t-${chunk.pair.id || idx}`}
                pair={chunk.pair}
                onOpenSubagent={onOpenSubagent}
                onEnter={(el) => openHover(el, chunk.pair)}
                onLeave={scheduleDismiss}
              />
            );
          }
          if (chunk.kind === "summary") {
            return (
              <div
                key={`s-${chunk.event.byte_offset}`}
                className="td__sub td__sub--summary"
              >
                <span className="td__sub-label">summary</span>
                <span>
                  {flattenContent(payloadOf(chunk.event).message?.content ?? "") ||
                    summaryTextOf(chunk.event)}
                </span>
              </div>
            );
          }
          if (chunk.kind === "system") {
            return (
              <div
                key={`sy-${chunk.event.byte_offset}`}
                className="td__sub td__sub--system"
              >
                <span className="td__sub-label">system</span>
                <span>
                  {payloadOf(chunk.event).subtype ?? "system"}{" "}
                  {flattenContent(payloadOf(chunk.event).message?.content ?? "")}
                </span>
              </div>
            );
          }
          return (
            <div
              key={`g-${chunk.event.byte_offset}`}
              className="td__sub td__sub--generic"
            >
              <span className="td__sub-label">{chunk.event.kind}</span>
              <details>
                <summary>raw</summary>
                <pre>{JSON.stringify(chunk.event.payload, null, 2)}</pre>
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

/** Walk turn events and emit render chunks. Consecutive assistant
 * events with no intervening visible tool (or summary/system/etc.)
 * merge into one assistant chunk so they render as one block with one
 * set of copy buttons. */
function buildChunks(
  turn: Turn,
  pairById: Map<string, ToolPair>,
  filters?: TimelineFilters,
): Chunk[] {
  const chunks: Chunk[] = [];
  let pending: TimelineEvent[] | null = null;

  const flushPending = () => {
    if (pending && pending.length > 0) {
      chunks.push({ kind: "assistant", events: pending });
    }
    pending = null;
  };

  for (const ev of turn.events) {
    if (ev.kind === "user" && ev === turn.userPrompt) continue;
    if (ev.kind === "user") continue; // tool_result wrapper — surfaced via pairs
    if (filters && !eventIsVisible(ev, filters)) continue;

    if (ev.kind === "assistant") {
      if (!pending) pending = [];
      pending.push(ev);

      const content = payloadOf(ev).message?.content;
      const toolUseIds = Array.isArray(content)
        ? (content as Array<{ type: string; id?: string }>)
            .filter((b) => b.type === "tool_use")
            .map((b) => b.id ?? "")
        : [];
      const visiblePairs = toolUseIds
        .map((id) => pairById.get(id))
        .filter(
          (p): p is ToolPair => !!p && (!filters || toolPairIsVisible(p, filters)),
        );

      if (visiblePairs.length > 0) {
        // Tools break the assistant run — emit the combined text first,
        // then each visible tool pair. The next assistant event will
        // start a fresh chunk.
        flushPending();
        for (const p of visiblePairs) chunks.push({ kind: "tool", pair: p });
      }
      continue;
    }

    flushPending();
    if (ev.kind === "summary") chunks.push({ kind: "summary", event: ev });
    else if (ev.kind === "system") chunks.push({ kind: "system", event: ev });
    else chunks.push({ kind: "generic", event: ev });
  }
  flushPending();
  return chunks;
}

function AssistantBlock({
  events,
  pairById,
  showThinking,
  onThinkingChip,
}: {
  events: TimelineEvent[];
  pairById: Map<string, ToolPair>;
  showThinking: boolean;
  onThinkingChip: (el: HTMLElement, text: string) => void;
}) {
  // Flatten content across all merged events.
  const allTexts: string[] = [];
  const usefulThoughts: Array<{ text: string; eventKey: number; idx: number }> = [];
  for (const ev of events) {
    for (const t of textBlocksIn(ev)) allTexts.push(t);
    const thoughts = thinkingBlocksIn(ev);
    thoughts.forEach((th, i) => {
      const text = typeof th.thinking === "string" ? th.thinking.trim() : "";
      if (text.length > 0) {
        usefulThoughts.push({ text, eventKey: ev.byte_offset, idx: i });
      }
    });
  }

  const copyResponseText = () =>
    events
      .map((ev) => formatAssistantText(ev))
      .filter((s) => s.length > 0)
      .join("\n\n");

  const copyEventText = () =>
    events
      .map((ev) => formatAssistantEvent(ev, pairById))
      .filter((s) => s.length > 0)
      .join("\n\n");

  const hasCopyable = allTexts.length > 0;

  return (
    <div className="td__sub td__sub--assistant">
      {hasCopyable && (
        <div className="td__assistant-actions" aria-label="Copy actions">
          <CopyButton
            getText={copyResponseText}
            label="text"
            icon="⧉"
            title="Copy just the assistant text as markdown"
          />
          <CopyButton
            getText={copyEventText}
            label="event"
            icon="⧉"
            title="Copy text + inline tool calls as markdown"
          />
        </div>
      )}
      {allTexts.map((t, i) => (
        <div key={`t-${i}`} className="td__text">
          <Markdown source={t} />
        </div>
      ))}
      {showThinking && usefulThoughts.length > 0 && (
        <div className="td__thinking-chips">
          {usefulThoughts.map((th, i) => (
            <button
              key={`k-${th.eventKey}-${th.idx}`}
              type="button"
              className="td__thinking-chip"
              onClick={(e: MouseEvent<HTMLButtonElement>) =>
                onThinkingChip(e.currentTarget, th.text)
              }
              title="View thinking"
            >
              💭 thinking
              {usefulThoughts.length > 1 ? ` ${i + 1}/${usefulThoughts.length}` : ""}
            </button>
          ))}
        </div>
      )}
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
  // ToolResultBlock.content is pre-flattened to a string by the
  // canonical parser; undefined means "tool produced no output".
  const body = r.content ?? "";
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
