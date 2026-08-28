import {
  type MouseEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { saveLibraryEntry } from "../../api/client";
import type { TimelineAssistantItem } from "../../api/types";
import { useMediaQuery } from "../../hooks/useMediaQuery";
import { appCommands } from "../../state/AppCommands";
import { MOBILE_LAYOUT_QUERY } from "../../state/displayPolicy";
import type { MenuItem } from "../common/contextMenuStore";
import {
  contextMenuTriggerProps,
  useContextMenu,
} from "../common/contextMenuStore";
import { copyToClipboard } from "../terminal/clipboard";
import { Icon } from "../../icons";
import type { ToolPair, Turn } from "./grouping";
import { Markdown } from "./Markdown";
import {
  formatAssistantItems,
  formatAssistantText,
  formatTurn,
  stripAssistantAnnotations,
} from "./markdown-export";
import { ThinkingFlyout } from "./ThinkingFlyout";
import { ToolHoverCard } from "./ToolHoverCard";
import { ToolCallRenderer } from "./tools/renderers";
import { Tooltip } from "../ui";
import "./TurnDetail.css";

interface Props {
  turn: Turn;
  showThinking: boolean;
  /** Hidden "user" speaker chip: replace the prompt body with an
   * explicit filtered label instead of showing it. */
  hideUserPrompt?: boolean;
  onOpenSubagent?: (pair: ToolPair) => void;
  /** Optional tool-call id to focus inside this turn. When set and
   * matched, that row renders expanded (others collapsed) and carries
   * a persistent outline; `focusKey` identifies the focus request so a
   * re-click on the same pair can re-trigger. */
  focusPairId?: string | null;
  focusKey?: string | null;
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

export function TurnDetail({
  turn,
  showThinking,
  hideUserPrompt = false,
  onOpenSubagent,
  focusPairId = null,
  focusKey = null,
}: Props) {
  const isMobile = useMediaQuery(MOBILE_LAYOUT_QUERY);
  const pairById = useMemo(
    () => new Map(turn.tool_pairs.map((pair) => [pair.id, pair] as const)),
    [turn.tool_pairs],
  );

  // Card expansion lives here, not in the rows: the newest card arrives
  // expanded and collapses again when the next one lands, manual toggles
  // stick, errors stay open, and a focus request rebases everything onto
  // the focused row. Manual choices reset when the selection moves to a
  // different turn but survive live refetches of the same turn.
  const [manualExpansion, setManualExpansion] = useState<Record<string, boolean>>({});
  const turnIdentity = `${turn.session_uuid ?? ""}:${turn.id}`;
  const seenTurnRef = useRef(turnIdentity);
  useEffect(() => {
    if (seenTurnRef.current !== turnIdentity) {
      seenTurnRef.current = turnIdentity;
      setManualExpansion({});
    }
  }, [turnIdentity]);
  const seenFocusRef = useRef<string | null>(null);
  useEffect(() => {
    if (focusKey == null || seenFocusRef.current === focusKey) return;
    seenFocusRef.current = focusKey;
    setManualExpansion({});
  }, [focusKey]);

  // Long live turns: stay pinned to the bottom while the reader is at
  // the bottom, hold position otherwise, and jump back to the top when
  // the selection moves to a different turn.
  const bodyRef = useRef<HTMLDivElement>(null);
  const atBottomRef = useRef(true);
  const onBodyScroll = useCallback(() => {
    const el = bodyRef.current;
    if (!el) return;
    atBottomRef.current = el.scrollTop + el.clientHeight >= el.scrollHeight - 48;
  }, []);
  const scrollTurnRef = useRef(turnIdentity);
  const growthKey = `${turn.event_count}:${turn.chunks.length}:${turn.tool_pairs.length}`;
  useLayoutEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    if (scrollTurnRef.current !== turnIdentity) {
      scrollTurnRef.current = turnIdentity;
      el.scrollTop = 0;
      atBottomRef.current = el.clientHeight >= el.scrollHeight - 48;
      return;
    }
    if (atBottomRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [turnIdentity, growthKey]);

  const focusActive = focusKey != null && focusPairId != null;
  const lastPairId =
    turn.tool_pairs.length > 0 ? turn.tool_pairs[turn.tool_pairs.length - 1]!.id : null;
  const isExpanded = useCallback(
    (pair: ToolPair): boolean => {
      const manual = manualExpansion[pair.id];
      if (manual !== undefined) return manual;
      if (isMobile) return false;
      if (focusActive) return pair.id === focusPairId;
      return pair.id === lastPairId || pair.is_error || pair.is_pending;
    },
    [manualExpansion, isMobile, focusActive, focusPairId, lastPairId],
  );
  const toggleExpansion = useCallback(
    (pair: ToolPair, currentlyExpanded: boolean) => {
      setManualExpansion((prev) => ({ ...prev, [pair.id]: !currentlyExpanded }));
    },
    [],
  );
  const [saveError, setSaveError] = useState<string | null>(null);
  const [thinking, setThinking] = useState<ThinkingAnchor | null>(null);
  const [hover, setHover] = useState<HoverAnchor | null>(null);
  const dismissTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const openCtx = useContextMenu((store) => store.open);
  const sessionBadge = useMemo(() => {
    const sessionUuid = turn.session_uuid?.trim();
    if (!sessionUuid) return null;
    const label =
      turn.session_label?.trim() ||
      turn.pty_session_id?.slice(0, 8) ||
      sessionUuid.slice(0, 8);
    const title = [
      turn.session_agent ?? "session",
      label,
      turn.session_state ?? "unknown",
      sessionUuid,
    ].join(" · ");
    return { label, title };
  }, [
    turn.session_uuid,
    turn.session_label,
    turn.pty_session_id,
    turn.session_agent,
    turn.session_state,
  ]);

  const savePrompt = useCallback(async () => {
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
  }, [turn.user_prompt_text]);

  const saveReference = useCallback(
    async (body: string, name: string) => {
      if (!body.trim()) return;
      try {
        await saveLibraryEntry("references", { name, body });
        appCommands.libraryChanged({ kind: "references" });
        setSaveError(null);
      } catch (err) {
        setSaveError(
          err instanceof Error ? err.message : "reference save failed",
        );
      }
    },
    [],
  );
  const saveReferenceFireAndForget = useCallback(
    (body: string, name: string) => void saveReference(body, name),
    [saveReference],
  );

  const openHover = useCallback(
    (el: HTMLElement, pair: ToolPair) => {
      if (isMobile) return;
      if (dismissTimer.current) clearTimeout(dismissTimer.current);
      setHover((prev) => {
        if (prev?.pinned && prev.pair.id === pair.id) return prev;
        return {
          el,
          pair,
          pinned: prev?.pinned && prev.pair.id === pair.id ? true : false,
        };
      });
    },
    [isMobile],
  );
  const scheduleDismiss = useCallback(() => {
    if (dismissTimer.current) clearTimeout(dismissTimer.current);
    dismissTimer.current = setTimeout(() => {
      setHover((prev) => (prev?.pinned ? prev : null));
    }, 180);
  }, []);

  const buildHeaderMenu = useCallback(
    () => [
      {
        kind: "item" as const,
        id: "copy-turn",
        label: "Copy turn as markdown",
        onSelect: () => {
          void copyToClipboard(formatTurn(turn));
        },
      },
    ],
    [turn],
  );
  const headerTriggerProps = useMemo(
    () => contextMenuTriggerProps(openCtx, buildHeaderMenu),
    [openCtx, buildHeaderMenu],
  );

  const buildPromptMenu = useCallback(() => {
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
  }, [turn.user_prompt_text, savePrompt]);
  const promptTriggerProps = useMemo(
    () => contextMenuTriggerProps(openCtx, buildPromptMenu),
    [openCtx, buildPromptMenu],
  );

  const onClearThinking = useCallback(() => setThinking(null), []);
  const onClearHover = useCallback(() => setHover(null), []);
  const onPinHover = useCallback(
    () => setHover((prev) => (prev ? { ...prev, pinned: true } : prev)),
    [],
  );
  const onThinkingChip = useCallback(
    (el: HTMLElement, text: string) => {
      setHover(null);
      setThinking({ el, text });
    },
    [],
  );
  const onHoverEnter = useCallback(() => {
    if (dismissTimer.current) clearTimeout(dismissTimer.current);
  }, []);

  return (
    <div className="td">
      <div
        className="td__header"
        aria-label="Turn actions"
        {...headerTriggerProps}
      >
        <div className="td__header-prompt">
          <span className="td__header-label">Prompt</span>
          <div
            className="td__prompt-text"
            aria-label="Prompt actions"
            {...promptTriggerProps}
          >
            {!turn.user_prompt_text ? (
              <span className="td__muted">(orphan turn — no user prompt)</span>
            ) : hideUserPrompt ? (
              <span className="td__muted">(user prompt hidden by filter)</span>
            ) : (
              <Markdown source={turn.user_prompt_text} />
            )}
          </div>
        </div>
        <div className="td__header-meta">
          {turn.is_sidechain && (
            <Tooltip label="Subagent (sidechain) turn">
              <span className="td__session-chip td__session-chip--sidechain">
                <Icon name="parent-session" size={12} /> subagent
              </span>
            </Tooltip>
          )}
          {sessionBadge && (
            <Tooltip label={sessionBadge.title}>
              <span className="td__session-chip">{sessionBadge.label}</span>
            </Tooltip>
          )}
          <span className="tabular">{turn.event_count} events</span>
          <span className="tabular">{turn.operation_count} tool calls</span>
          {turn.duration_ms > 0 && (
            <Tooltip label="Turn duration">
              <span className="tabular">{formatDurationMs(turn.duration_ms)}</span>
            </Tooltip>
          )}
          {((turn.input_tokens ?? 0) > 0 || (turn.output_tokens ?? 0) > 0) && (
            <Tooltip
              label={`${(turn.input_tokens ?? 0).toLocaleString()} input tokens (incl. cache) · ${(turn.output_tokens ?? 0).toLocaleString()} output tokens`}
            >
              <span className="tabular">
                ↑{formatTokens(turn.input_tokens ?? 0)} ↓
                {formatTokens(turn.output_tokens ?? 0)}
              </span>
            </Tooltip>
          )}
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

      <div
        className="td__body"
        data-testid="turn-detail"
        ref={bodyRef}
        onScroll={onBodyScroll}
      >
        {turn.chunks.map((chunk, idx) => {
          if (chunk.kind === "assistant") {
            return (
              <AssistantBlock
                key={`a-${idx}`}
                items={chunk.items}
                thinking={chunk.thinking}
                pairById={pairById}
                showThinking={showThinking}
                onSaveReference={saveReferenceFireAndForget}
                onThinkingChip={onThinkingChip}
              />
            );
          }

          if (chunk.kind === "tool") {
            const pair = pairById.get(chunk.pair_id);
            if (!pair) return null;
            const isFocused =
              focusPairId != null && pair.id === focusPairId;
            return (
              <ToolPairRow
                key={`t-${pair.id || idx}`}
                pair={pair}
                expanded={isExpanded(pair)}
                onToggle={toggleExpansion}
                onOpenSubagent={onOpenSubagent}
                onEnter={openHover}
                onLeave={scheduleDismiss}
                hoverEnabled={!isMobile}
                isFocused={isFocused}
                focusToken={focusKey}
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
          onClose={onClearThinking}
        />
      )}
      {hover && !isMobile && (
        <ToolHoverCard
          anchor={hover.el}
          pair={hover.pair}
          pinned={hover.pinned}
          onPin={onPinHover}
          onClose={onClearHover}
          onMouseEnter={onHoverEnter}
          onMouseLeave={scheduleDismiss}
        />
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
  const texts = useMemo(
    () =>
      items.flatMap((item) =>
        item.kind === "text" ? [stripAssistantAnnotations(item.text)] : [],
      ),
    [items],
  );
  const hasCopyable = texts.length > 0;
  const fullBody = useMemo(
    () => formatAssistantItems(items, pairById),
    [items, pairById],
  );
  const name = useMemo(
    () => defaultReferenceName(formatAssistantText(items) || fullBody),
    [items, fullBody],
  );
  const openCtx = useContextMenu((store) => store.open);
  const buildAssistantMenu = useCallback((): MenuItem[] | null => {
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
  }, [hasCopyable, fullBody, items, name, onSaveReference]);
  const triggerProps = useMemo(
    () => contextMenuTriggerProps(openCtx, buildAssistantMenu),
    [openCtx, buildAssistantMenu],
  );

  return (
    <div
      className="td__sub td__sub--assistant"
      aria-label="Assistant block actions"
      {...triggerProps}
    >
      {texts.map((text, idx) => (
        <div key={`t-${idx}`} className="td__text">
          <Markdown source={text} />
        </div>
      ))}
      {showThinking && thinking.length > 0 && (
        <div className="td__thinking-chips">
          {thinking.map((text, idx) => (
            <ThinkingChip
              key={`k-${idx}`}
              text={text}
              index={idx}
              total={thinking.length}
              onOpen={onThinkingChip}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function ThinkingChip({
  text,
  index,
  total,
  onOpen,
}: {
  text: string;
  index: number;
  total: number;
  onOpen: (el: HTMLElement, text: string) => void;
}) {
  const onClick = useCallback(
    (e: MouseEvent<HTMLButtonElement>) => onOpen(e.currentTarget, text),
    [onOpen, text],
  );
  return (
    <button
      type="button"
      className="td__thinking-chip"
      onClick={onClick}
      aria-label="View thinking"
    >
      <Icon name="sparkles" size={12} />
      <span>thinking</span>
      {total > 1 ? (
        <span className="tabular">
          {index + 1}/{total}
        </span>
      ) : null}
    </button>
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
  expanded,
  onToggle,
  onOpenSubagent,
  onEnter,
  onLeave,
  hoverEnabled,
  isFocused,
  focusToken,
}: {
  pair: ToolPair;
  /** Controlled by TurnDetail: newest-card default, sticky manual
   * toggles, focus rebasing. */
  expanded: boolean;
  onToggle: (pair: ToolPair, currentlyExpanded: boolean) => void;
  onOpenSubagent?: (pair: ToolPair) => void;
  onEnter: (el: HTMLElement, pair: ToolPair) => void;
  onLeave: () => void;
  hoverEnabled: boolean;
  isFocused: boolean;
  focusToken: string | null;
}) {
  const rowRef = useRef<HTMLDivElement>(null);
  const enterTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const appliedFocusTokenRef = useRef<string | null>(null);

  // A new focus request scrolls the focused row into view; the
  // expansion rebase itself happens in TurnDetail.
  useEffect(() => {
    if (focusToken == null) return;
    if (appliedFocusTokenRef.current === focusToken) return;
    appliedFocusTokenRef.current = focusToken;
    if (isFocused && rowRef.current) {
      rowRef.current.scrollIntoView({ block: "center", behavior: "auto" });
    }
  }, [focusToken, isFocused]);

  useEffect(
    () => () => {
      if (enterTimer.current) clearTimeout(enterTimer.current);
    },
    [],
  );

  const handleEnter = useCallback(() => {
    if (!hoverEnabled) return;
    if (enterTimer.current) clearTimeout(enterTimer.current);
    enterTimer.current = setTimeout(() => {
      if (rowRef.current) onEnter(rowRef.current, pair);
    }, 500);
  }, [hoverEnabled, onEnter, pair]);
  const handleLeave = useCallback(() => {
    if (enterTimer.current) {
      clearTimeout(enterTimer.current);
      enterTimer.current = null;
    }
    onLeave();
  }, [onLeave]);
  const toggleExpanded = useCallback(
    () => onToggle(pair, expanded),
    [onToggle, pair, expanded],
  );
  const onOpenSubagentClick = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      if (onOpenSubagent) onOpenSubagent(pair);
    },
    [onOpenSubagent, pair],
  );
  const toolProp = useMemo(
    () => ({
      id: pair.id,
      name: pair.name,
      operationType: pair.operation_type,
      input: pair.input,
      resultPayload: pair.result?.payload,
      fileTouches: pair.file_touches,
    }),
    [
      pair.id,
      pair.name,
      pair.operation_type,
      pair.input,
      pair.result?.payload,
      pair.file_touches,
    ],
  );

  return (
    <div
      ref={rowRef}
      className={`td__tool ${pair.is_error ? "td__tool--error" : ""} ${
        pair.is_pending ? "td__tool--pending" : ""
      } ${isFocused ? "td__tool--focused" : ""}`}
      data-testid="tool-pair-row"
      data-tool-type={toolType(pair)}
      data-pair-id={pair.id}
      data-focused={isFocused ? "true" : undefined}
    >
      <div className="td__tool-header">
        <button
          type="button"
          className="td__tool-toggle"
          onClick={toggleExpanded}
          onMouseEnter={hoverEnabled ? handleEnter : undefined}
          onMouseLeave={hoverEnabled ? handleLeave : undefined}
          onFocus={hoverEnabled ? handleEnter : undefined}
          onBlur={hoverEnabled ? handleLeave : undefined}
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
            onClick={onOpenSubagentClick}
          >
            View agent log →
          </button>
        )}
      </div>
      {expanded && (
        <div className="td__tool-body">
          <ToolCallRenderer tool={toolProp} />
          {pair.result && <ToolResultRender pair={pair} />}
        </div>
      )}
    </div>
  );
}

function ToolResultRender({ pair }: { pair: ToolPair }) {
  const result = pair.result!;
  if (!result.content && result.payload && usesStructuredResult(pair)) {
    return null;
  }
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
  // Anything that canonicalised into `file_edits` — including a
  // code-mode exec that only applies a patch — summarizes by its first
  // edited path when nothing more specific matches below.
  const firstEditedPath = (): string => {
    const edits = input.file_edits;
    if (!Array.isArray(edits) || edits.length === 0) return "";
    const first = edits[0] as Record<string, unknown>;
    return typeof first.path === "string" ? first.path : "";
  };
  switch (toolType(pair)) {
    case "edit":
    case "write":
    case "multi_edit":
    case "apply_patch":
    case "read":
      return pick("path") ?? firstEditedPath();
    case "bash":
    case "exec":
    case "exec_command":
      // The agent's own description ("Run backend gates") reads better
      // collapsed than the raw command line; codex exec carries none,
      // so its command stays the summary.
      return (
        pick("description") ??
        pick("command") ??
        pick("cmd") ??
        firstEditedPath()
      ).slice(0, 120);
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
      return pick("description") ?? pick("command") ?? pick("cmd") ?? "";
  }
}

function usesStructuredResult(pair: ToolPair): boolean {
  const kind = toolType(pair);
  return kind === "edit" || kind === "multi_edit";
}

function toolType(pair: ToolPair): string {
  return pair.operation_type ?? pair.name;
}

function formatDurationMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const minutes = Math.floor(seconds / 60);
  const remainder = Math.round(seconds - minutes * 60);
  return remainder > 0 ? `${minutes}m${remainder}s` : `${minutes}m`;
}

function formatTokens(count: number): string {
  if (count < 1000) return `${count}`;
  if (count < 1_000_000) return `${(count / 1000).toFixed(1)}k`;
  return `${(count / 1_000_000).toFixed(1)}M`;
}
