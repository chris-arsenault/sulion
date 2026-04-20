import { type MouseEvent, useMemo, useRef, useState } from "react";

import { saveLibraryEntry } from "../../api/client";
import type { TimelineAssistantItem } from "../../api/types";
import { appCommands } from "../../state/AppCommands";
import type { MenuItem } from "../common/ContextMenu";
import {
  contextMenuHandler,
  useContextMenu,
} from "../common/ContextMenu";
import { copyToClipboard } from "../terminal/clipboard";
import { Icon } from "../../icons";
import type { ToolPair, Turn } from "./grouping";
import { Markdown } from "./Markdown";
import {
  formatAssistantItems,
  formatAssistantText,
  formatTurn,
} from "./markdown-export";
import { ThinkingFlyout } from "./ThinkingFlyout";
import { ToolHoverCard } from "./ToolHoverCard";
import { ToolCallRenderer } from "./tools/renderers";
import "./TurnDetail.css";

interface Props {
  turn: Turn;
  showThinking: boolean;
  onOpenSubagent?: (pair: ToolPair) => void;
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

export function TurnDetail({ turn, showThinking, onOpenSubagent }: Props) {
  const pairById = useMemo(
    () => new Map(turn.tool_pairs.map((pair) => [pair.id, pair] as const)),
    [turn.tool_pairs],
  );
  const [saveError, setSaveError] = useState<string | null>(null);
  const [thinking, setThinking] = useState<ThinkingAnchor | null>(null);
  const [hover, setHover] = useState<HoverAnchor | null>(null);
  const dismissTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const openCtx = useContextMenu((store) => store.open);

  const savePrompt = async () => {
    const body = turn.user_prompt_text?.trim();
    if (!body) return;
    try {
      await saveLibraryEntry("prompts", {
        name: defaultPromptName(body),
        body,
      });
      appCommands.libraryChanged({ kind: "prompts" });
      setSaveError(null);
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : "prompt save failed");
    }
  };

  const saveReference = async (body: string, name: string) => {
    if (!body.trim()) return;
    try {
      await saveLibraryEntry("references", {
        name,
        body,
      });
      appCommands.libraryChanged({ kind: "references" });
      setSaveError(null);
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : "reference save failed");
    }
  };

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

  const onHeaderContextMenu = contextMenuHandler(openCtx, () => [
    {
      kind: "item",
      id: "copy-turn",
      label: "Copy turn as markdown",
      onSelect: () => {
        void copyToClipboard(formatTurn(turn));
      },
    },
  ]);

  const onPromptContextMenu = contextMenuHandler(openCtx, () => {
    const body = turn.user_prompt_text?.trim();
    if (!body) return null;
    const items: MenuItem[] = [
      {
        kind: "item",
        id: "copy-prompt",
        label: "Copy prompt",
        onSelect: () => {
          void copyToClipboard(body);
        },
      },
      {
        kind: "item",
        id: "save-prompt",
        label: "Save as prompt",
        onSelect: () => void savePrompt(),
      },
    ];
    return items;
  });

  return (
    <div className="td">
      <div className="td__header" onContextMenu={onHeaderContextMenu}>
        <div className="td__header-prompt">
          <span className="td__header-label">Prompt</span>
          <div
            className="td__prompt-text"
            onContextMenu={onPromptContextMenu}
          >
            {turn.user_prompt_text ? (
              <Markdown source={turn.user_prompt_text} />
            ) : (
              <span className="td__muted">(orphan turn — no user prompt)</span>
            )}
          </div>
        </div>
        <div className="td__header-meta">
          <span className="tabular">{turn.event_count} events</span>
          <span className="tabular">{turn.operation_count} tool calls</span>
          {turn.thinking_count > 0 && showThinking && (
            <span className="td__thinking-tally">
              <Icon name="sparkles" size={12} />
              <span className="tabular">{turn.thinking_count}</span>
            </span>
          )}
          {turn.has_errors && (
            <span className="td__errors">
              <Icon name="alert-triangle" size={12} /> errors
            </span>
          )}
        </div>
        {saveError && <div className="td__save-error">save failed: {saveError}</div>}
      </div>

      <div className="td__body" data-testid="turn-detail">
        {turn.chunks.map((chunk, idx) => {
          if (chunk.kind === "assistant") {
            return (
              <AssistantBlock
                key={`a-${idx}`}
                items={chunk.items}
                thinking={chunk.thinking}
                pairById={pairById}
                showThinking={showThinking}
                onSaveReference={(body, name) => void saveReference(body, name)}
                onThinkingChip={(el, text) => {
                  setHover(null);
                  setThinking({ el, text });
                }}
              />
            );
          }

          if (chunk.kind === "tool") {
            const pair = pairById.get(chunk.pair_id);
            if (!pair) return null;
            return (
              <ToolPairRow
                key={`t-${pair.id || idx}`}
                pair={pair}
                onOpenSubagent={onOpenSubagent}
                onEnter={(el) => openHover(el, pair)}
                onLeave={scheduleDismiss}
              />
            );
          }

          if (chunk.kind === "summary") {
            return (
              <div key={`s-${idx}`} className="td__sub td__sub--summary">
                <span className="td__sub-label">summary</span>
                <span>{chunk.text}</span>
              </div>
            );
          }

          if (chunk.kind === "system") {
            return (
              <div key={`sy-${idx}`} className="td__sub td__sub--system">
                <span className="td__sub-label">system</span>
                <span>
                  {chunk.subtype ?? "system"} {chunk.text}
                </span>
              </div>
            );
          }

          return (
            <div key={`g-${idx}`} className="td__sub td__sub--generic">
              <span className="td__sub-label">{chunk.label}</span>
              <details>
                <summary>details</summary>
                <pre>{JSON.stringify(chunk.details, null, 2)}</pre>
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

function AssistantBlock({
  items,
  thinking,
  pairById,
  showThinking,
  onSaveReference,
  onThinkingChip,
}: {
  items: TimelineAssistantItem[];
  thinking: string[];
  pairById: Map<string, ToolPair>;
  showThinking: boolean;
  onSaveReference: (body: string, name: string) => void;
  onThinkingChip: (el: HTMLElement, text: string) => void;
}) {
  const texts = items.flatMap((item) => (item.kind === "text" ? [item.text] : []));
  const hasCopyable = texts.length > 0;
  const fullBody = formatAssistantItems(items, pairById);
  const name = defaultReferenceName(formatAssistantText(items) || fullBody);
  const openCtx = useContextMenu((store) => store.open);
  const onContextMenu = contextMenuHandler(openCtx, () => {
    const menu: MenuItem[] = [];
    if (hasCopyable) {
      menu.push({
        kind: "item",
        id: "copy-text",
        label: "Copy text",
        onSelect: () => {
          void copyToClipboard(formatAssistantText(items));
        },
      });
    }
    if (fullBody) {
      menu.push({
        kind: "item",
        id: "copy-event",
        label: "Copy event",
        onSelect: () => {
          void copyToClipboard(fullBody);
        },
      });
      menu.push({
        kind: "item",
        id: "save-reference",
        label: "Save as reference",
        onSelect: () => onSaveReference(fullBody, name),
      });
    }
    return menu.length > 0 ? menu : null;
  });

  return (
    <div
      className="td__sub td__sub--assistant"
      onContextMenu={onContextMenu}
    >
      {texts.map((text, idx) => (
        <div key={`t-${idx}`} className="td__text">
          <Markdown source={text} />
        </div>
      ))}
      {showThinking && thinking.length > 0 && (
        <div className="td__thinking-chips">
          {thinking.map((text, idx) => (
            <button
              key={`k-${idx}`}
              type="button"
              className="td__thinking-chip"
              onClick={(e: MouseEvent<HTMLButtonElement>) =>
                onThinkingChip(e.currentTarget, text)
              }
              aria-label="View thinking"
            >
              <Icon name="sparkles" size={12} />
              <span>thinking</span>
              {thinking.length > 1 ? (
                <span className="tabular">
                  {idx + 1}/{thinking.length}
                </span>
              ) : null}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function defaultPromptName(text: string): string {
  return defaultReferenceName(text).replace(/^Reference:\s*/, "Prompt: ");
}

function defaultReferenceName(text: string): string {
  const firstLine = text
    .split("\n")
    .map((line) => line.trim())
    .find(Boolean);
  if (!firstLine) return "Reference";
  return `Reference: ${firstLine.slice(0, 64)}`;
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
  const lowSignal = !pair.is_error && !pair.is_pending;
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
      className={`td__tool ${pair.is_error ? "td__tool--error" : ""} ${
        pair.is_pending ? "td__tool--pending" : ""
      }`}
      onMouseEnter={handleEnter}
      onMouseLeave={handleLeave}
      data-testid="tool-pair-row"
    >
      <div className="td__tool-header">
        <button
          type="button"
          className="td__tool-toggle"
          onClick={() => setExpanded((value) => !value)}
          aria-label={expanded ? "Collapse tool details" : "Expand tool details"}
        >
          <span
            className={
              expanded
                ? "td__tool-chevron td__tool-chevron--open"
                : "td__tool-chevron"
            }
          >
            <Icon name="chevron-right" size={12} />
          </span>
          <span
            className={`td__tool-name td__tool-name--${toolType(pair).toLowerCase()}`}
          >
            {toolType(pair)}
          </span>
          <span className="td__tool-summary">{toolSummary(pair)}</span>
          {pair.is_pending && <span className="td__tool-status">pending</span>}
          {pair.is_error && (
            <span className="td__tool-status td__tool-status--error">error</span>
          )}
          {!expanded && !pair.is_error && !pair.is_pending && (
            <span className="td__tool-status td__tool-status--ok">ok</span>
          )}
        </button>
        {toolType(pair) === "task" && pair.subagent && onOpenSubagent && (
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
            tool={{
              id: pair.id,
              name: pair.name,
              operationType: pair.operation_type,
              input: pair.input,
              fileTouches: pair.file_touches,
            }}
          />
          {pair.result && <ToolResultRender pair={pair} />}
        </div>
      )}
    </div>
  );
}

function ToolResultRender({ pair }: { pair: ToolPair }) {
  const result = pair.result!;
  const body = result.content ?? "";
  const truncated =
    body.length > 2000 ? `${body.slice(0, 2000)}\n… (${body.length} chars)` : body;
  return (
    <div className={`td__tool-result ${result.is_error ? "td__tool-result--error" : ""}`}>
      <div className="td__tool-result-label">
        result{result.is_error ? " (error)" : ""}
      </div>
      <pre>{truncated || "(empty result)"}</pre>
    </div>
  );
}

function toolSummary(pair: ToolPair): string {
  const input = (pair.input ?? {}) as Record<string, unknown>;
  const pick = (key: string) =>
    typeof input[key] === "string" ? (input[key] as string) : undefined;
  switch (toolType(pair)) {
    case "edit":
    case "write":
    case "multi_edit":
    case "read":
      return pick("path") ?? "";
    case "bash":
    case "exec_command":
      return (pick("command") ?? pick("cmd") ?? "").slice(0, 120);
    case "grep":
    case "glob":
      return pick("pattern") ?? "";
    case "task":
      return pick("description") ?? pick("agent") ?? "";
    case "todo_write":
      return "todos updated";
    case "web_fetch":
      return pick("url") ?? "";
    case "web_search":
      return pick("query") ?? "";
    default:
      return "";
  }
}

function toolType(pair: ToolPair): string {
  return pair.operation_type ?? pair.name;
}
