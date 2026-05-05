import { useCallback, useEffect, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { getMonitorTimeline } from "../api/client";
import type {
  MonitorSessionTurn,
  MonitorTimelineRequest,
  MonitorTimelineResponse,
  TimelineTurn,
} from "../api/types";
import { Icon } from "../icons";
import { useSessions } from "../state/SessionStore";
import { useTabs } from "../state/TabStore";
import { FilterChips } from "./timeline/FilterChips";
import { useTimelineFilters } from "./timeline/filters";
import { Markdown } from "./timeline/Markdown";
import "./MonitorPane.css";

export function MonitorPane({ active = true }: { active?: boolean }) {
  const [data, setData] = useState<MonitorTimelineResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const filterHook = useTimelineFilters();
  const { filters } = filterHook;
  const sessionIds = useMonitorSessionIds();
  const sessionRevisionKey = useSessions((store) =>
    JSON.stringify(
      sessionIds.map((id) => {
        const session = store.sessions.find((candidate) => candidate.id === id);
        return [
          id,
          session?.timeline_revision ?? 0,
          session?.last_event_at ?? null,
          session?.current_session_uuid ?? null,
          session?.agent_runtime?.state ?? "none",
        ];
      }),
    ),
  );

  const request = useMemo<MonitorTimelineRequest>(
    () => ({
      session_ids: sessionIds,
      hidden_speakers: Array.from(filters.hiddenSpeakers),
      hidden_operation_categories: Array.from(filters.hiddenOperationCategories),
      errors_only: filters.errorsOnly,
      show_bookkeeping: filters.showBookkeeping,
      show_sidechain: filters.showSidechain,
      file_path: filters.filePath || undefined,
    }),
    [filters, sessionIds],
  );
  const requestKey = useMemo(() => JSON.stringify(request), [request]);

  useEffect(() => {
    if (!active) return;
    if (sessionIds.length === 0) {
      setData(null);
      setError(null);
      setLoading(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    const load = async () => {
      try {
        const next = await getMonitorTimeline(request);
        if (cancelled) return;
        setData(next);
        setError(null);
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "monitor fetch failed");
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, [active, request, requestKey, sessionIds.length, sessionRevisionKey]);

  return (
    <div className="monitor-pane" data-testid="monitor-pane">
      <header className="monitor-pane__header">
        <div>
          <div className="monitor-pane__eyebrow">Monitor</div>
          <h2>Active agent output</h2>
        </div>
        <div className="monitor-pane__status">
          {loading ? "refreshing" : `${data?.sessions.length ?? sessionIds.length} sessions`}
        </div>
      </header>
      <FilterChips {...filterHook} />
      {error ? <div className="monitor-pane__error">{error}</div> : null}
      {sessionIds.length === 0 ? (
        <div className="monitor-pane__empty">
          Open terminal or timeline tabs to monitor their latest agent output.
        </div>
      ) : (
        <div className="monitor-pane__list">
          {(data?.sessions ?? []).map((item) => (
            <MonitorCard key={item.pty_session_id} item={item} />
          ))}
        </div>
      )}
    </div>
  );
}

function useMonitorSessionIds(): string[] {
  return useTabs(
    useShallow((store) => {
      const seen = new Set<string>();
      const out: string[] = [];
      for (const pane of ["top", "bottom"] as const) {
        for (const tabId of store.panes[pane]) {
          const tab = store.tabs[tabId];
          if (!tab?.sessionId) continue;
          if (tab.kind !== "terminal" && tab.kind !== "timeline") continue;
          if (seen.has(tab.sessionId)) continue;
          seen.add(tab.sessionId);
          out.push(tab.sessionId);
        }
      }
      return out;
    }),
  );
}

function MonitorCard({ item }: { item: MonitorSessionTurn }) {
  const openTab = useTabs((store) => store.openTab);
  const label = item.label?.trim() || item.pty_session_id.slice(0, 8);
  const assistant = item.turn ? latestAssistantText(item.turn) : null;
  const prompt = item.turn?.user_prompt_text?.trim() || item.turn?.preview || "";
  const openTimeline = useCallback(() => {
    if (!item.turn) {
      openTab({ kind: "timeline", sessionId: item.pty_session_id }, "bottom");
      return;
    }
    openTab(
      {
        kind: "timeline",
        sessionId: item.pty_session_id,
        focusTurnId: item.turn.id,
        focusKey: crypto.randomUUID(),
      },
      "bottom",
    );
  }, [item.pty_session_id, item.turn, openTab]);

  return (
    <article className="monitor-card">
      <button
        type="button"
        className="monitor-card__head"
        onClick={openTimeline}
        aria-label={`Open timeline for ${label}`}
      >
        <span className="monitor-card__session">
          <Icon name="activity" size={14} />
          <span>{label}</span>
        </span>
        <span className="monitor-card__repo">{item.repo}</span>
        <span className="monitor-card__time">
          {item.turn ? shortTime(item.turn.end_timestamp) : "no turns"}
        </span>
        {item.current_session_agent ? (
          <span className="monitor-card__agent">
            {item.current_session_agent} · {item.total_event_count} events
          </span>
        ) : (
          <span className="monitor-card__agent">No agent session</span>
        )}
        {item.turn && item.turn.operation_count > 0 ? (
          <span className="monitor-card__tools">
            {item.turn.operation_count} tool call{item.turn.operation_count === 1 ? "" : "s"}
            {item.turn.has_errors ? " · errors" : ""}
          </span>
        ) : null}
      </button>
      <div className="monitor-card__body">
        <div className="monitor-card__assistant">
          {assistant ? (
            <Markdown source={assistant} />
          ) : (
            <span className="monitor-card__muted">
              {item.turn
                ? "No assistant text after current filters."
                : "Waiting for transcript data."}
            </span>
          )}
        </div>
        {prompt ? (
          <div className="monitor-card__prompt">
            <span>Prompt</span>
            <p>{prompt}</p>
          </div>
        ) : null}
      </div>
    </article>
  );
}

function latestAssistantText(turn: TimelineTurn): string | null {
  for (let i = turn.chunks.length - 1; i >= 0; i -= 1) {
    const chunk = turn.chunks[i];
    if (chunk?.kind !== "assistant") continue;
    const text = chunk.items
      .filter((item) => item.kind === "text")
      .map((item) => item.text.trim())
      .filter(Boolean)
      .join("\n\n");
    if (text) return text;
  }
  return null;
}

function shortTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
