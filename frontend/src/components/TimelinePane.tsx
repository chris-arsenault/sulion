// Renders backend-projected timeline summaries and selected turn detail.
// The unified app-state poll carries timeline revision markers; this
// pane fetches summaries only when its active resource revision changes.
// On narrow viewports the detail view becomes an overlay modal instead of
// a side pane.
//
// The inspector's TurnDetail is reused by the SubagentModal so drill-in
// into sidechain logs renders the same way.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type MouseEvent as ReactMouseEvent,
  type MutableRefObject,
} from "react";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";

import {
  getRepoTimeline,
  getRepoTimelineTurn,
  getTimeline,
  getTimelineTurn,
  sendSessionPrompt,
  startSessionAgent,
} from "../api/client";
import type {
  AgentLaunchType,
  SessionView,
  TimelineQuery,
  TimelineSubagent,
  TimelineSummaryResponse,
} from "../api/types";
import { useMediaQuery } from "../hooks/useMediaQuery";
import {
  clampTimelineFontScale,
  TIMELINE_FONT_SCALE_DEFAULT,
  TIMELINE_FONT_SCALE_MAX,
  TIMELINE_FONT_SCALE_MIN,
  TIMELINE_FONT_SCALE_STEP,
  useTimelineFontScale,
} from "../state/paneTextScale";
import { useSessions } from "../state/SessionStore";
import { useTabs } from "../state/TabStore";
import { FilterChips } from "./timeline/FilterChips";
import { useTimelineFilters } from "./timeline/filters";
import { type ToolPair, type Turn, type TurnSummary } from "./timeline/grouping";
import { SessionInspectorPane } from "./timeline/SessionInspectorPane";
import { SubagentModal } from "./timeline/SubagentModal";
import { TurnRow } from "./timeline/TurnRow";
import { Tooltip } from "./ui";
import "./TimelinePane.css";

const INSPECTOR_WIDTH_KEY = "sulion.timeline.inspector.width.v1";
const DEFAULT_INSPECTOR_FRACTION = 0.55;
const MIN_INSPECTOR_FRACTION = 0.28;
const MAX_INSPECTOR_FRACTION = 0.78;

interface CachedTurnDetail {
  fingerprint: string;
  turn: Turn;
}

export function TimelinePane({
  tabId,
  sessionId,
  repo,
  active = true,
  focusTurnId,
  focusPairId,
  focusKey,
}: {
  tabId?: string;
  sessionId?: string;
  repo?: string;
  active?: boolean;
  focusTurnId?: number;
  focusPairId?: string;
  focusKey?: string;
}) {
  const [timeline, setTimeline] = useState<TimelineSummaryResponse | null>(null);
  const [detailCache, setDetailCache] = useState<Map<string, CachedTurnDetail>>(
    () => new Map(),
  );
  const [currentSessionUuid, setCurrentSessionUuid] = useState<string | null>(null);
  const [currentSessionAgent, setCurrentSessionAgent] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const virtuoso = useRef<VirtuosoHandle | null>(null);
  const [subagent, setSubagent] = useState<TimelineSubagent | null>(null);
  const [selectedTurnKey, setSelectedTurnKey] = useState<string | null>(null);
  const appliedFocusKeyRef = useRef<string | null>(null);
  const loadedSummaryKeyRef = useRef<string | null>(null);

  const filterHook = useTimelineFilters();
  const { filters } = filterHook;
  const narrow = useMediaQuery("(max-width: 999px)");
  const [timelineFontScale, setTimelineFontScale] = useTimelineFontScale();
  const resourceRevision = useSessions((store) => {
    if (sessionId) {
      return store.sessions.find((session) => session.id === sessionId)?.timeline_revision ?? 0;
    }
    if (repo) {
      return store.repos.find((candidate) => candidate.name === repo)?.timeline_revision ?? 0;
    }
    return 0;
  });
  const session = useSessions((store) =>
    sessionId ? store.sessions.find((candidate) => candidate.id === sessionId) : undefined,
  );
  const refreshSessions = useSessions((store) => store.refresh);

  const [inspectorFraction, setInspectorFraction] = useState<number>(() => {
    if (typeof window === "undefined") return DEFAULT_INSPECTOR_FRACTION;
    const raw = window.localStorage.getItem(INSPECTOR_WIDTH_KEY);
    const n = raw ? Number(raw) : NaN;
    if (Number.isFinite(n) && n >= MIN_INSPECTOR_FRACTION && n <= MAX_INSPECTOR_FRACTION) {
      return n;
    }
    return DEFAULT_INSPECTOR_FRACTION;
  });
  useEffect(() => {
    window.localStorage.setItem(INSPECTOR_WIDTH_KEY, String(inspectorFraction));
  }, [inspectorFraction]);

  const query = useMemo<TimelineQuery>(
    () => ({
      hidden_speakers: Array.from(filters.hiddenSpeakers),
      hidden_operation_categories: Array.from(filters.hiddenOperationCategories),
      errors_only: filters.errorsOnly,
      show_bookkeeping: filters.showBookkeeping,
      show_sidechain: filters.showSidechain,
      file_path: filters.filePath || undefined,
    }),
    [filters],
  );
  const queryKey = useMemo(
    () =>
      JSON.stringify({
        hidden_speakers: [...filters.hiddenSpeakers].sort(),
        hidden_operation_categories: [...filters.hiddenOperationCategories].sort(),
        errors_only: filters.errorsOnly,
        show_bookkeeping: filters.showBookkeeping,
        show_sidechain: filters.showSidechain,
        file_path: filters.filePath,
      }),
    [filters],
  );

  useEffect(() => {
    setTimeline(null);
    setCurrentSessionUuid(null);
    setCurrentSessionAgent(null);
    setLoadError(null);
    setDetailError(null);
    setDetailCache(new Map());
    setSubagent(null);
    setSelectedTurnKey(null);
    appliedFocusKeyRef.current = null;
    loadedSummaryKeyRef.current = null;
  }, [sessionId, repo]);

  useEffect(() => {
    setDetailCache(new Map());
    setDetailError(null);
  }, [queryKey]);

  useEffect(() => {
    if (!active || (!sessionId && !repo)) return;
    const summaryKey = `${sessionId ?? ""}:${repo ?? ""}:${resourceRevision}:${queryKey}`;
    if (loadedSummaryKeyRef.current === summaryKey) return;
    let cancelled = false;
    const load = async () => {
      if (cancelled) return;
      try {
        const resp = sessionId
          ? await getTimeline(sessionId, query)
          : await getRepoTimeline(repo!, query);
        if (cancelled) return;
        loadedSummaryKeyRef.current = summaryKey;
        setCurrentSessionUuid(resp.session_uuid);
        setCurrentSessionAgent(resp.session_agent);
        setTimeline(resp);
        setLoadError(null);
      } catch (err) {
        if (!cancelled) {
          setLoadError(err instanceof Error ? err.message : "timeline fetch failed");
        }
      }
    };

    void load();
    return () => {
      cancelled = true;
    };
  }, [active, sessionId, repo, resourceRevision, query, queryKey]);

  const turns = useMemo<TurnSummary[]>(
    () => timeline?.turns ?? [],
    [timeline],
  );

  // Apply a focus request exactly once per focusKey. `turns` stays in
  // deps so we retry across revision updates if the target turn hasn't been
  // ingested yet, but the ref guard prevents later summary refreshes
  // from stomping on a selection the user has since moved.
  useEffect(() => {
    if (focusTurnId == null || !focusKey) return;
    if (appliedFocusKeyRef.current === focusKey) return;
    const exists = turns.findIndex((turn) => turn.id === focusTurnId);
    if (exists === -1) return;
    appliedFocusKeyRef.current = focusKey;
    setSelectedTurnKey(turnIdentity(turns[exists]!));
    virtuoso.current?.scrollToIndex({
      index: exists,
      align: "center",
      behavior: "auto",
    });
  }, [focusKey, focusTurnId, turns]);

  const selectedSummary = useMemo<TurnSummary | null>(
    () =>
      selectedTurnKey == null
        ? null
        : turns.find((t) => turnIdentity(t) === selectedTurnKey) ?? null,
    [selectedTurnKey, turns],
  );
  const selectedTurn = useMemo<Turn | null>(
    () =>
      selectedTurnKey == null || selectedSummary == null
        ? null
        : detailCache.get(selectedTurnKey)?.turn ?? null,
    [detailCache, selectedSummary, selectedTurnKey],
  );
  const selectedFingerprint = selectedSummary
    ? turnSummaryFingerprint(selectedSummary)
    : null;
  const detailPending =
    selectedSummary != null && selectedTurn == null && !detailError;

  useEffect(() => {
    if (!active || !selectedSummary || !selectedTurnKey) return;
    if (selectedFingerprint == null) return;
    if (detailCache.get(selectedTurnKey)?.fingerprint === selectedFingerprint) return;
    if (!sessionId && (!repo || !selectedSummary.session_uuid)) return;

    let cancelled = false;
    const fetchDetail = async () => {
      try {
        const resp = sessionId
          ? await getTimelineTurn(sessionId, selectedSummary.id, query)
          : await getRepoTimelineTurn(
              repo!,
              selectedSummary.session_uuid!,
              selectedSummary.id,
              query,
            );
        if (cancelled) return;
        setDetailCache((prev) => {
          if (prev.get(selectedTurnKey)?.fingerprint === selectedFingerprint) {
            return prev;
          }
          const next = new Map(prev);
          next.set(selectedTurnKey, {
            fingerprint: selectedFingerprint,
            turn: resp.turn,
          });
          return next;
        });
        setDetailError(null);
      } catch (err) {
        if (!cancelled) {
          setDetailError(err instanceof Error ? err.message : "turn fetch failed");
        }
      }
    };

    void fetchDetail();
    return () => {
      cancelled = true;
    };
  }, [
    detailCache,
    active,
    query,
    repo,
    selectedFingerprint,
    selectedSummary,
    selectedTurnKey,
    sessionId,
  ]);

  const handleSubagent = useCallback((pair: ToolPair) => {
    if (pair.subagent) setSubagent(pair.subagent);
  }, []);
  const closeSubagent = useCallback(() => setSubagent(null), []);

  // A manual click in the turn list is the user overriding whatever
  // focus the tab was opened with. Strip the focus fields from the
  // tab so later polls (or tab revisits) don't re-apply them — and
  // so the persistent focus outline on a tool row goes away. Also
  // drops follow-latest mode, since the user picking a specific turn
  // contradicts "keep snapping to the newest one".
  const clearTimelineFocus = useTabs((store) => store.clearTimelineFocus);
  const { setFollowLatest } = filterHook;
  const handleTurnSelect = useCallback(
    (key: string) => {
      setSelectedTurnKey(key);
      if (tabId) clearTimelineFocus(tabId);
      if (filters.followLatest) setFollowLatest(false);
    },
    [tabId, clearTimelineFocus, filters.followLatest, setFollowLatest],
  );

  // Follow-latest: while the filter is on, keep the selection pinned
  // to the most recently arrived turn across summary refreshes. Turn identity is
  // stable, so we only restart the selection when the last-turn key
  // actually changes — avoids fighting unrelated re-renders.
  useEffect(() => {
    if (!filters.followLatest) return;
    const last = turns[turns.length - 1];
    if (!last) return;
    const lastKey = turnIdentity(last);
    setSelectedTurnKey((prev) => (prev === lastKey ? prev : lastKey));
    virtuoso.current?.scrollToIndex({
      index: turns.length - 1,
      align: "end",
      behavior: "auto",
    });
  }, [filters.followLatest, turns]);

  const onDividerMouseDown = useCallback(
    (e: ReactMouseEvent<HTMLDivElement>) => {
      e.preventDefault();
      const container = (e.target as HTMLElement).parentElement;
      if (!container) return;
      const rect = container.getBoundingClientRect();
      const onMove = (ev: MouseEvent) => {
        const fraction = (ev.clientX - rect.left) / rect.width;
        const listFraction = Math.max(
          1 - MAX_INSPECTOR_FRACTION,
          Math.min(1 - MIN_INSPECTOR_FRACTION, fraction),
        );
        setInspectorFraction(1 - listFraction);
      };
      const onUp = () => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
      };
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    },
    [],
  );

  const onDividerKeyDown = useCallback((e: React.KeyboardEvent) => {
    const step = e.shiftKey ? 0.1 : 0.03;
    if (e.key === "ArrowLeft") {
      e.preventDefault();
      setInspectorFraction((v) => Math.min(MAX_INSPECTOR_FRACTION, v + step));
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      setInspectorFraction((v) => Math.max(MIN_INSPECTOR_FRACTION, v - step));
    }
  }, []);

  const listFraction = 1 - inspectorFraction;
  const empty = turns.length === 0;

  const clearSelectedTurn = useCallback(() => setSelectedTurnKey(null), []);
  const paneStyle = useMemo(
    (): CSSProperties =>
      ({
        "--timeline-t-meta": `calc(var(--t-meta) * ${timelineFontScale})`,
        "--timeline-t-ui": `calc(var(--t-ui) * ${timelineFontScale})`,
        "--timeline-t-body": `calc(var(--t-body) * ${timelineFontScale})`,
      }) as CSSProperties,
    [timelineFontScale],
  );
  const splitStyle = useMemo(
    (): CSSProperties => ({
      gridTemplateColumns: `${listFraction}fr 6px ${inspectorFraction}fr`,
    }),
    [listFraction, inspectorFraction],
  );

  const decreaseTimelineText = useCallback(() => {
    setTimelineFontScale((value) =>
      clampTimelineFontScale(value - TIMELINE_FONT_SCALE_STEP),
    );
  }, [setTimelineFontScale]);

  const increaseTimelineText = useCallback(() => {
    setTimelineFontScale((value) =>
      clampTimelineFontScale(value + TIMELINE_FONT_SCALE_STEP),
    );
  }, [setTimelineFontScale]);

  const resetTimelineText = useCallback(() => {
    setTimelineFontScale(TIMELINE_FONT_SCALE_DEFAULT);
  }, [setTimelineFontScale]);

  return (
    <div
      className="timeline-pane"
      data-testid="timeline-pane"
      // eslint-disable-next-line local/no-inline-styles -- pane-scoped text variables are user preferences, not theme classes
      style={paneStyle}
    >
      <div className="timeline-pane__header">
        <span className="timeline-pane__title">Timeline</span>
        {repo ? (
          <span className="timeline-pane__scope">repo {repo}</span>
        ) : currentSessionUuid ? (
          <Tooltip label={`${currentSessionAgent ?? "session"} ${currentSessionUuid}`}>
            <span className="timeline-pane__session">
              {(currentSessionAgent ?? "session")} {currentSessionUuid.slice(0, 8)}
            </span>
          </Tooltip>
        ) : null}
        <span className="timeline-pane__count tabular">
          {turns.length} turn{turns.length === 1 ? "" : "s"} · {timeline?.total_event_count ?? 0} events
        </span>
        {(loadError || detailError) && (
          <Tooltip label={loadError ?? detailError ?? ""}>
            <span className="timeline-pane__error">error</span>
          </Tooltip>
        )}
        <div className="timeline-pane__text-controls" aria-label="Timeline text size controls">
          <span className="timeline-pane__text-label">text</span>
          <button
            type="button"
            className="timeline-pane__text-button"
            onClick={decreaseTimelineText}
            disabled={timelineFontScale <= TIMELINE_FONT_SCALE_MIN}
            aria-label="Decrease timeline text size"
          >
            A-
          </button>
          <button
            type="button"
            className="timeline-pane__text-button timeline-pane__text-button--value"
            onClick={resetTimelineText}
            aria-label="Reset timeline text size"
          >
            {Math.round(timelineFontScale * 100)}%
          </button>
          <button
            type="button"
            className="timeline-pane__text-button"
            onClick={increaseTimelineText}
            disabled={timelineFontScale >= TIMELINE_FONT_SCALE_MAX}
            aria-label="Increase timeline text size"
          >
            A+
          </button>
        </div>
      </div>
      <FilterChips {...filterHook} />
      {empty ? (
        <div className="timeline-pane__empty">
          {(timeline?.total_event_count ?? 0) === 0
            ? repo
              ? "No timeline data for this repo yet."
              : currentSessionUuid
              ? "Waiting for events…"
              : "No transcript session correlated yet."
            : "No turns match current filters."}
        </div>
      ) : narrow ? (
        <>
          <div className="timeline-pane__list-narrow">
            <TurnList
              turns={turns}
              selectedTurnKey={selectedTurnKey}
              showThinking={filters.showThinking}
              onSelect={handleTurnSelect}
              virtuosoRef={virtuoso}
            />
          </div>
          <SessionInspectorPane
            turn={selectedTurn}
            loading={detailPending}
            showThinking={filters.showThinking}
            onOpenSubagent={handleSubagent}
            asOverlay
            onClose={clearSelectedTurn}
            focusPairId={focusPairId ?? null}
            focusKey={focusKey ?? null}
          />
        </>
      ) : (
        <div
          className="timeline-pane__split"
          // eslint-disable-next-line local/no-inline-styles -- resizable split fractions are per-user-drag; can't be CSS classes
          style={splitStyle}
        >
          <div className="timeline-pane__list">
            <TurnList
              turns={turns}
              selectedTurnKey={selectedTurnKey}
              showThinking={filters.showThinking}
              onSelect={handleTurnSelect}
              virtuosoRef={virtuoso}
            />
          </div>
          <div
            className="timeline-pane__divider"
            role="slider"
            aria-orientation="vertical"
            aria-label="Resize inspector"
            aria-valuemin={Math.round(MIN_INSPECTOR_FRACTION * 100)}
            aria-valuemax={Math.round(MAX_INSPECTOR_FRACTION * 100)}
            aria-valuenow={Math.round(inspectorFraction * 100)}
            tabIndex={0}
            onMouseDown={onDividerMouseDown}
            onKeyDown={onDividerKeyDown}
          />
          <SessionInspectorPane
            turn={selectedTurn}
            loading={detailPending}
            showThinking={filters.showThinking}
            onOpenSubagent={handleSubagent}
            asOverlay={false}
            focusPairId={focusPairId ?? null}
            focusKey={focusKey ?? null}
          />
        </div>
      )}
      {sessionId && (
        <TimelinePromptBar
          sessionId={sessionId}
          session={session}
          onRefresh={refreshSessions}
        />
      )}
      {subagent && (
        <SubagentModal
          subagent={subagent}
          showThinking={filters.showThinking}
          onClose={closeSubagent}
        />
      )}
    </div>
  );
}

function TimelinePromptBar({
  sessionId,
  session,
  onRefresh,
}: {
  sessionId: string;
  session?: SessionView;
  onRefresh: () => Promise<void>;
}) {
  const [text, setText] = useState("");
  const [pending, setPending] = useState<"send" | AgentLaunchType | null>(null);
  const [error, setError] = useState<string | null>(null);
  const runtime = session?.agent_runtime ?? {
    agent: null,
    state: "none",
    started_at: null,
    ended_at: null,
    exit_code: null,
  };
  const metadata = session?.agent_metadata ?? null;
  const live = session?.state === "live";
  const running = live && runtime.state === "running";
  const starting = live && runtime.state === "starting";
  const canLaunch = live && !starting && runtime.state !== "running";
  const canSend = running && text.trim().length > 0 && pending == null;
  const status = promptStatusText(session ?? null, runtime);
  const meta = promptMetadataText(metadata);

  const startAgent = useCallback(
    async (agent: AgentLaunchType) => {
      if (!canLaunch || pending) return;
      setPending(agent);
      setError(null);
      try {
        await startSessionAgent(sessionId, agent);
        await onRefresh();
      } catch (err) {
        setError(err instanceof Error ? err.message : "failed to start agent");
      } finally {
        setPending(null);
      }
    },
    [canLaunch, onRefresh, pending, sessionId],
  );

  const sendPrompt = useCallback(async () => {
    if (!canSend) return;
    const prompt = text;
    setPending("send");
    setError(null);
    try {
      await sendSessionPrompt(sessionId, prompt);
      setText("");
      await onRefresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : "failed to send prompt");
    } finally {
      setPending(null);
    }
  }, [canSend, onRefresh, sessionId, text]);

  const onTextChange = useCallback(
    (e: React.ChangeEvent<HTMLTextAreaElement>) => setText(e.target.value),
    [],
  );
  const onTextKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        void sendPrompt();
      }
    },
    [sendPrompt],
  );
  const onSendClick = useCallback(() => {
    void sendPrompt();
  }, [sendPrompt]);
  const onStartClaude = useCallback(() => {
    void startAgent("claude");
  }, [startAgent]);
  const onStartCodex = useCallback(() => {
    void startAgent("codex");
  }, [startAgent]);

  return (
    <div className="timeline-prompt" aria-label="Agent prompt controls">
      <div className="timeline-prompt__status">
        <span>{status}</span>
        {meta && <span className="timeline-prompt__meta">{meta}</span>}
        {error && <span className="timeline-prompt__error">{error}</span>}
      </div>
      {running ? (
        <div className="timeline-prompt__input-row">
          <textarea
            value={text}
            onChange={onTextChange}
            onKeyDown={onTextKeyDown}
            placeholder="Type a prompt. Ctrl+Enter sends to the running agent."
            rows={2}
            className="timeline-prompt__textarea"
            aria-label="Prompt text"
            disabled={pending != null}
          />
          <button
            type="button"
            className="timeline-prompt__button timeline-prompt__button--primary"
            onClick={onSendClick}
            disabled={!canSend}
          >
            {pending === "send" ? "Sending…" : "Send"}
          </button>
        </div>
      ) : (
        <div className="timeline-prompt__launch-row">
          <button
            type="button"
            className="timeline-prompt__button"
            onClick={onStartClaude}
            disabled={!canLaunch || pending != null}
          >
            {pending === "claude" ? "Starting…" : "Start Claude"}
          </button>
          <button
            type="button"
            className="timeline-prompt__button"
            onClick={onStartCodex}
            disabled={!canLaunch || pending != null}
          >
            {pending === "codex" ? "Starting…" : "Start Codex"}
          </button>
        </div>
      )}
    </div>
  );
}

function promptStatusText(
  session: SessionView | null,
  runtime: NonNullable<SessionView["agent_runtime"]>,
): string {
  if (!session) return "Loading session state…";
  if (session.state !== "live") return `Session is ${session.state}`;
  const agent = runtime.agent ? agentDisplayName(runtime.agent) : "agent";
  switch (runtime.state) {
    case "running":
      return `${agent} is running`;
    case "starting":
      return `${agent} is starting`;
    case "exited": {
      const exitCode = runtime.exit_code == null ? "" : ` (${runtime.exit_code})`;
      return `${agent} exited${exitCode}`;
    }
    case "none":
    default:
      return "No agent running in this PTY";
  }
}

function promptMetadataText(metadata: SessionView["agent_metadata"]): string | null {
  if (!metadata) return null;
  const bits = [
    metadata.model,
    metadata.reasoning_effort ? `effort ${metadata.reasoning_effort}` : null,
    metadata.model_provider,
  ].filter(Boolean);
  return bits.length ? bits.join(" · ") : null;
}

function agentDisplayName(agent: string): string {
  if (agent === "claude" || agent === "claude-code") return "Claude";
  if (agent === "codex") return "Codex";
  return agent;
}

function turnKey(_i: number, t: TurnSummary): string {
  return turnIdentity(t);
}

function turnIdentity(turn: Turn | TurnSummary): string {
  return turn.turn_key ?? `${turn.id}`;
}

function turnSummaryFingerprint(turn: TurnSummary): string {
  return [
    turn.end_timestamp,
    turn.duration_ms,
    turn.event_count,
    turn.operation_count,
    turn.thinking_count,
    turn.has_errors,
  ].join(":");
}

function TurnList({
  turns,
  selectedTurnKey,
  showThinking,
  onSelect,
  virtuosoRef,
}: {
  turns: TurnSummary[];
  selectedTurnKey: string | null;
  showThinking: boolean;
  onSelect: (key: string) => void;
  virtuosoRef: MutableRefObject<VirtuosoHandle | null>;
}) {
  const renderItem = useCallback(
    (_i: number, t: TurnSummary) => (
      <TurnRow
        turn={t}
        selected={selectedTurnKey === turnIdentity(t)}
        showThinking={showThinking}
        onSelect={onSelect}
      />
    ),
    [selectedTurnKey, showThinking, onSelect],
  );
  return (
    <Virtuoso
      ref={virtuosoRef}
      data={turns}
      computeItemKey={turnKey}
      itemContent={renderItem}
      followOutput="smooth"
      className="timeline-pane__virtuoso"
    />
  );
}
