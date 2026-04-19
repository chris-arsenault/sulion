// Polls /api/sessions/:id/history → filters → groups into turns →
// renders a compact list on the left and the selected turn's detail
// on the right (ticket #28). On narrow viewports the detail view
// becomes an overlay modal instead of a side pane.
//
// The inspector's TurnDetail is reused by the SubagentModal so drill-in
// into sidechain logs renders the same way.

import { useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent, type MutableRefObject } from "react";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";

import { getHistory } from "../api/client";
import type { TimelineEvent } from "../api/types";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { FilterChips } from "./timeline/FilterChips";
import {
  eventMatchesFilters,
  useTimelineFilters,
} from "./timeline/filters";
import {
  groupIntoTurns,
  prefilter,
  type ToolPair,
  type Turn,
} from "./timeline/grouping";
import { SessionInspectorPane } from "./timeline/SessionInspectorPane";
import { SubagentModal } from "./timeline/SubagentModal";
import { TurnRow } from "./timeline/TurnRow";
import "./TimelinePane.css";

const POLL_MS = 1500;
const INSPECTOR_WIDTH_KEY = "shuttlecraft.timeline.inspector.width.v1";
const DEFAULT_INSPECTOR_FRACTION = 0.55;
const MIN_INSPECTOR_FRACTION = 0.28;
const MAX_INSPECTOR_FRACTION = 0.78;

interface SubagentSelection {
  toolUseId: string;
  seedUuid?: string;
  title: string;
}

export function TimelinePane({ sessionId }: { sessionId: string }) {
  const [events, setEvents] = useState<TimelineEvent[]>([]);
  const [claudeSession, setClaudeSession] = useState<string | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);
  const offsetRef = useRef<number>(-1);
  const virtuoso = useRef<VirtuosoHandle | null>(null);
  const [subagent, setSubagent] = useState<SubagentSelection | null>(null);
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

  useEffect(() => {
    offsetRef.current = -1;
    setEvents([]);
    setClaudeSession(null);
    setLastError(null);
    setSubagent(null);
    setSelectedTurnId(null);

    let cancelled = false;
    let inFlight = false;
    const tick = async () => {
      if (cancelled || inFlight) return;
      inFlight = true;
      try {
        const resp = await getHistory(sessionId, {
          after: offsetRef.current >= 0 ? offsetRef.current : undefined,
        });
        if (cancelled) return;
        setClaudeSession(resp.claude_session_uuid);
        if (resp.events.length > 0) {
          setEvents((prev) => [...prev, ...resp.events]);
          offsetRef.current = resp.events[resp.events.length - 1].byte_offset;
        }
        setLastError(null);
      } catch (err) {
        if (!cancelled) {
          setLastError(err instanceof Error ? err.message : "history fetch failed");
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
  }, [sessionId]);

  const turns = useMemo<Turn[]>(() => {
    const prefiltered = prefilter(events, {
      showBookkeeping: filters.showBookkeeping,
      showSidechain: filters.showSidechain,
    });
    const grouped = groupIntoTurns(prefiltered);
    const hasActiveFacet =
      filters.speakers.size > 0 ||
      filters.tools.size > 0 ||
      filters.errorsOnly ||
      filters.filePath.length > 0;
    if (!hasActiveFacet) return grouped;
    return grouped.filter((t) =>
      t.events.some((ev) => eventMatchesFilters(ev, filters)),
    );
  }, [events, filters]);

  const selectedTurn = useMemo<Turn | null>(
    () =>
      selectedTurnId == null
        ? null
        : turns.find((t) => t.id === selectedTurnId) ?? null,
    [selectedTurnId, turns],
  );

  const handleSubagent = (pair: ToolPair) => {
    if (!pair.id) return;
    setSubagent({
      toolUseId: pair.id,
      seedUuid:
        typeof (pair.useEvent.payload as { uuid?: string } | null)?.uuid === "string"
          ? (pair.useEvent.payload as { uuid: string }).uuid
          : undefined,
      title: subagentTitleFromPair(pair),
    });
  };

  const onDividerMouseDown = (e: ReactMouseEvent<HTMLDivElement>) => {
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
  };

  const listFraction = 1 - inspectorFraction;
  const empty = turns.length === 0;

  return (
    <div className="timeline-pane" data-testid="timeline-pane">
      <div className="timeline-pane__header">
        <span className="timeline-pane__title">Timeline</span>
        {claudeSession && (
          <span
            className="timeline-pane__session"
            title={`claude session ${claudeSession}`}
          >
            claude {claudeSession.slice(0, 8)}
          </span>
        )}
        <span className="timeline-pane__count">
          {turns.length} turn{turns.length === 1 ? "" : "s"} · {events.length} events
        </span>
        {lastError && (
          <span className="timeline-pane__error" title={lastError}>
            error
          </span>
        )}
      </div>
      <FilterChips {...filterHook} />
      {empty ? (
        <div className="timeline-pane__empty">
          {events.length === 0
            ? claudeSession
              ? "Waiting for events…"
              : "No Claude session correlated yet. Start `claude` in the terminal."
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
            onClose={() => setSelectedTurnId(null)}
          />
        </>
      ) : (
        <div
          className="timeline-pane__split"
          style={{
            gridTemplateColumns: `${listFraction}fr 6px ${inspectorFraction}fr`,
          }}
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
            role="separator"
            aria-orientation="vertical"
            onMouseDown={onDividerMouseDown}
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
          toolUseId={subagent.toolUseId}
          seedUuid={subagent.seedUuid}
          title={subagent.title}
          allEvents={events}
          showThinking={filters.showThinking}
          onClose={() => setSubagent(null)}
        />
      )}
    </div>
  );
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
  return (
    <Virtuoso
      ref={virtuosoRef}
      data={turns}
      computeItemKey={(_i, t) => `${t.id}`}
      itemContent={(_i, t) => (
        <TurnRow
          turn={t}
          selected={selectedTurnId === t.id}
          showThinking={showThinking}
          onSelect={() => onSelect(t.id)}
        />
      )}
      followOutput="smooth"
      className="timeline-pane__virtuoso"
    />
  );
}

function subagentTitleFromPair(pair: ToolPair): string {
  const input = (pair.input ?? {}) as Record<string, unknown>;
  const desc = typeof input.description === "string" ? input.description : null;
  const agent =
    typeof input.subagent_type === "string" ? input.subagent_type : null;
  if (desc) return `Agent log · ${desc}`;
  if (agent) return `Agent log · ${agent}`;
  return "Agent log";
}
