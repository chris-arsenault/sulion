// Polls the backend's timeline projection and renders a compact turn
// list on the left and the selected turn's detail on the right. On
// narrow viewports the detail view becomes an overlay modal instead of
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
  type MouseEvent as ReactMouseEvent,
  type MutableRefObject,
} from "react";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";

import { getTimeline } from "../api/client";
import type { TimelineQuery, TimelineResponse, TimelineSubagent } from "../api/types";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { FilterChips } from "./timeline/FilterChips";
import { useTimelineFilters } from "./timeline/filters";
import { type ToolPair, type Turn } from "./timeline/grouping";
import { SessionInspectorPane } from "./timeline/SessionInspectorPane";
import { SubagentModal } from "./timeline/SubagentModal";
import { TurnRow } from "./timeline/TurnRow";
import { Tooltip } from "./ui";
import "./TimelinePane.css";

const POLL_MS = 1500;
const INSPECTOR_WIDTH_KEY = "sulion.timeline.inspector.width.v1";
const DEFAULT_INSPECTOR_FRACTION = 0.55;
const MIN_INSPECTOR_FRACTION = 0.28;
const MAX_INSPECTOR_FRACTION = 0.78;

export function TimelinePane({
  sessionId,
  focusTurnId,
  focusKey,
}: {
  sessionId: string;
  focusTurnId?: number;
  focusKey?: string;
}) {
  const [timeline, setTimeline] = useState<TimelineResponse | null>(null);
  const [currentSessionUuid, setCurrentSessionUuid] = useState<string | null>(null);
  const [currentSessionAgent, setCurrentSessionAgent] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const virtuoso = useRef<VirtuosoHandle | null>(null);
  const [subagent, setSubagent] = useState<TimelineSubagent | null>(null);
  const [selectedTurnId, setSelectedTurnId] = useState<number | null>(null);

  const filterHook = useTimelineFilters();
  const { filters } = filterHook;
  const narrow = useMediaQuery("(max-width: 999px)");

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
    setSubagent(null);
    setSelectedTurnId(null);
  }, [sessionId]);

  useEffect(() => {
    let cancelled = false;
    let inFlight = false;
    const tick = async () => {
      if (cancelled || inFlight) return;
      inFlight = true;
      try {
        const resp = await getTimeline(sessionId, query);
        if (cancelled) return;
        setCurrentSessionUuid(resp.session_uuid);
        setCurrentSessionAgent(resp.session_agent);
        setTimeline(resp);
        setLoadError(null);
      } catch (err) {
        if (!cancelled) {
          setLoadError(err instanceof Error ? err.message : "timeline fetch failed");
        }
      } finally {
        inFlight = false;
      }
    };

    void tick();
    const timer = setInterval(() => void tick(), POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [sessionId, query, queryKey]);

  const turns = useMemo<Turn[]>(
    () => timeline?.turns ?? [],
    [timeline],
  );

  useEffect(() => {
    if (focusTurnId == null) return;
    const exists = turns.findIndex((turn) => turn.id === focusTurnId);
    if (exists === -1) return;
    setSelectedTurnId(focusTurnId);
    virtuoso.current?.scrollToIndex({
      index: exists,
      align: "center",
      behavior: "auto",
    });
  }, [focusKey, focusTurnId, turns]);

  const selectedTurn = useMemo<Turn | null>(
    () =>
      selectedTurnId == null
        ? null
        : turns.find((t) => t.id === selectedTurnId) ?? null,
    [selectedTurnId, turns],
  );

  const handleSubagent = useCallback((pair: ToolPair) => {
    if (pair.subagent) setSubagent(pair.subagent);
  }, []);
  const closeSubagent = useCallback(() => setSubagent(null), []);

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

  const clearSelectedTurn = useCallback(() => setSelectedTurnId(null), []);
  const splitStyle = useMemo(
    () => ({
      gridTemplateColumns: `${listFraction}fr 6px ${inspectorFraction}fr`,
    }),
    [listFraction, inspectorFraction],
  );

  return (
    <div className="timeline-pane" data-testid="timeline-pane">
      <div className="timeline-pane__header">
        <span className="timeline-pane__title">Timeline</span>
        {currentSessionUuid && (
          <Tooltip label={`${currentSessionAgent ?? "session"} ${currentSessionUuid}`}>
            <span className="timeline-pane__session">
              {(currentSessionAgent ?? "session")} {currentSessionUuid.slice(0, 8)}
            </span>
          </Tooltip>
        )}
        <span className="timeline-pane__count tabular">
          {turns.length} turn{turns.length === 1 ? "" : "s"} · {timeline?.total_event_count ?? 0} events
        </span>
        {loadError && (
          <Tooltip label={loadError}>
            <span className="timeline-pane__error">error</span>
          </Tooltip>
        )}
      </div>
      <FilterChips {...filterHook} />
      {empty ? (
        <div className="timeline-pane__empty">
          {(timeline?.total_event_count ?? 0) === 0
            ? currentSessionUuid
              ? "Waiting for events…"
              : "No transcript session correlated yet."
            : "No turns match current filters."}
        </div>
      ) : narrow ? (
        <>
          <div className="timeline-pane__list-narrow">
            <TurnList
              turns={turns}
              selectedTurnId={selectedTurnId}
              showThinking={filters.showThinking}
              onSelect={setSelectedTurnId}
              virtuosoRef={virtuoso}
            />
          </div>
          <SessionInspectorPane
            turn={selectedTurn}
            showThinking={filters.showThinking}
            onOpenSubagent={handleSubagent}
            asOverlay
            onClose={clearSelectedTurn}
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
              selectedTurnId={selectedTurnId}
              showThinking={filters.showThinking}
              onSelect={setSelectedTurnId}
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
            showThinking={filters.showThinking}
            onOpenSubagent={handleSubagent}
            asOverlay={false}
          />
        </div>
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

function turnKey(_i: number, t: Turn): string {
  return `${t.id}`;
}

function TurnList({
  turns,
  selectedTurnId,
  showThinking,
  onSelect,
  virtuosoRef,
}: {
  turns: Turn[];
  selectedTurnId: number | null;
  showThinking: boolean;
  onSelect: (id: number) => void;
  virtuosoRef: MutableRefObject<VirtuosoHandle | null>;
}) {
  const renderItem = useCallback(
    (_i: number, t: Turn) => (
      <TurnRow
        turn={t}
        selected={selectedTurnId === t.id}
        showThinking={showThinking}
        onSelect={onSelect}
      />
    ),
    [selectedTurnId, showThinking, onSelect],
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
