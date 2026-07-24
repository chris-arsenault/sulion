import { useCallback, useEffect, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { getMonitorTimeline } from "../api/client";
import type {
  MonitorSessionTurn,
  MonitorTimelineRequest,
  MonitorTimelineResponse,
  PlanSummaryView,
  SessionActivityState,
  SessionView,
  TimelineTurn,
} from "../api/types";
import { Icon } from "../icons";
import { useSessions } from "../state/SessionStore";
import { type TabStore, useTabs } from "../state/TabStore";
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
  const { sessions, plans } = useSessions(
    useShallow((store) => ({
      sessions: store.sessions,
      plans: store.plans,
    })),
  );
  const liveSessions = useMemo(
    () =>
      sessions
        .filter((session) => session.state === "live")
        .sort(sessionActivityCompare),
    [sessions],
  );
  const sessionIds = useMemo(
    () => liveSessions.map((session) => session.id),
    [liveSessions],
  );
  const sessionRevisionKey = useMemo(
    () =>
      JSON.stringify(
        liveSessions.map((session) => [
          session.id,
          session.timeline_revision ?? 0,
          session.last_event_at ?? null,
          session.current_session_uuid ?? null,
          session.agent_runtime?.state ?? "none",
          session.activity?.state ?? "unknown",
          session.activity?.updated_at ?? null,
          session.current_plan?.revision ?? null,
          session.agent_usage?.updated_at ?? null,
        ]),
      ),
    [liveSessions],
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

  const turnBySession = useMemo(
    () =>
      new Map(
        (data?.sessions ?? []).map((session) => [
          session.pty_session_id,
          session,
        ]),
      ),
    [data],
  );
  const counts = useMemo(() => activityCounts(liveSessions), [liveSessions]);
  const teams = useMemo(
    () => buildTeams(liveSessions, plans),
    [liveSessions, plans],
  );
  const contextWatchCount = useMemo(
    () =>
      liveSessions.filter((session) => {
        const context = contextHealth(session);
        return context?.level === "warning" || context?.level === "critical";
      }).length,
    [liveSessions],
  );

  return (
    <div className="monitor-pane" data-testid="monitor-pane">
      <header className="monitor-pane__header">
        <div>
          <div className="monitor-pane__eyebrow">Engineering portfolio</div>
          <h2>Team overview</h2>
          <p>Live staffing, current assignments, and agent health by repository.</p>
        </div>
        <div className="monitor-pane__status">
          {loading ? "refreshing · " : ""}
          updated from live terminal telemetry
        </div>
      </header>
      <div className="monitor-pane__summary" aria-label="Portfolio health summary">
        <PortfolioMetric label="Teams" value={teams.length} />
        <PortfolioMetric label="Open terminals" value={liveSessions.length} />
        <PortfolioMetric
          label="Working"
          value={counts.working}
          tone="working"
        />
        <PortfolioMetric
          label="Need attention"
          value={counts.attention}
          tone={counts.attention > 0 ? "critical" : "muted"}
        />
        <PortfolioMetric
          label="Context watch"
          value={contextWatchCount}
          tone={contextWatchCount > 0 ? "warning" : "muted"}
        />
      </div>
      <FilterChips {...filterHook} />
      {error ? <div className="monitor-pane__error">{error}</div> : null}
      {teams.length === 0 ? (
        <div className="monitor-pane__empty">
          No live teams or published plans.
        </div>
      ) : (
        <div className="monitor-pane__teams">
          {teams.map((team) => (
            <TeamSection
              key={team.repo}
              team={team}
              turnBySession={turnBySession}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function PortfolioMetric({
  label,
  value,
  tone = "default",
}: {
  label: string;
  value: number;
  tone?: "default" | "working" | "warning" | "critical" | "muted";
}) {
  return (
    <div className={`portfolio-metric portfolio-metric--${tone}`}>
      <strong>{value}</strong>
      <span>{label}</span>
    </div>
  );
}

interface MonitorTeam {
  repo: string;
  sessions: SessionView[];
  plans: PlanSummaryView[];
}

function TeamSection({
  team,
  turnBySession,
}: {
  team: MonitorTeam;
  turnBySession: Map<string, MonitorSessionTurn>;
}) {
  const counts = activityCounts(team.sessions);
  const staffed = team.sessions.filter(
    (session) => session.agent_runtime?.state === "running",
  ).length;
  return (
    <section className="monitor-team" aria-label={`${team.repo} team`}>
      <header className="monitor-team__header">
        <div className="monitor-team__identity">
          <span className="monitor-team__mark" aria-hidden="true">
            {team.repo.slice(0, 2).toUpperCase()}
          </span>
          <div>
            <h3>{team.repo}</h3>
            <p>
              {staffed} engineer{staffed === 1 ? "" : "s"} ·{" "}
              {team.sessions.length} open terminal
              {team.sessions.length === 1 ? "" : "s"}
            </p>
          </div>
        </div>
        <div className="monitor-team__status">
          {counts.attention > 0 ? (
            <span className="activity-badge activity-badge--needs_input">
              {counts.attention} need attention
            </span>
          ) : null}
          <span className="activity-badge activity-badge--working">
            {counts.working} working
          </span>
          {team.plans.length > 0 ? (
            <span className="activity-badge">
              {team.plans.length} open plan{team.plans.length === 1 ? "" : "s"}
            </span>
          ) : null}
        </div>
      </header>
      {team.plans.length > 0 ? <TeamPlans plans={team.plans} /> : null}
      {team.sessions.length === 0 ? (
        <div className="monitor-team__unassigned">
          Published work is open, but no terminal is staffed on this team.
        </div>
      ) : (
        <div className="monitor-team__engineers">
          {team.sessions.map((session) => (
            <MonitorCard
              key={session.id}
              session={session}
              item={turnBySession.get(session.id) ?? null}
            />
          ))}
        </div>
      )}
    </section>
  );
}

function MonitorCard({
  session,
  item,
}: {
  session: SessionView;
  item: MonitorSessionTurn | null;
}) {
  const openTab = useTabs((store) => store.openTab);
  const label = session.label?.trim() || session.id.slice(0, 8);
  const assistant = item?.turn ? latestAssistantText(item.turn) : null;
  const prompt =
    item?.turn?.user_prompt_text?.trim() || item?.turn?.preview || "";
  const state = operationalState(session);
  const activitySummary =
    session.activity?.summary ||
    session.activity?.reason ||
    defaultActivitySummary(state);
  const context = contextHealth(session);
  const health = engineerHealth(session, context);
  const uptimeStart =
    session.agent_runtime?.started_at ?? session.created_at;
  const uptime = elapsedDuration(uptimeStart);
  const totalTokens = session.agent_usage?.total_tokens ?? null;
  const burnRate = averageTokensPerHour(totalTokens, uptimeStart);
  const agent = session.current_session_agent ?? session.agent_runtime?.agent;
  const model = session.agent_metadata?.model;
  const openTimeline = useCallback(() => {
    if (!item?.turn) {
      openTab({ kind: "timeline", sessionId: session.id }, "bottom");
      return;
    }
    openTab(
      {
        kind: "timeline",
        sessionId: session.id,
        focusTurnId: item.turn.id,
        focusKey: crypto.randomUUID(),
      },
      "bottom",
    );
  }, [item?.turn, openTab, session.id]);
  const openPlan = useCallback(() => {
    if (!session.current_plan) return;
    openTab({
      kind: "plan",
      repo: session.repo,
      planId: session.current_plan.id,
    });
  }, [openTab, session.current_plan, session.repo]);

  return (
    <article className={`monitor-card monitor-card--${health.level}`}>
      <header className="monitor-card__head">
        <button
          type="button"
          className="monitor-card__identity"
          onClick={openTimeline}
          aria-label={`Open timeline for ${label}`}
        >
          <span className="monitor-card__avatar">
            <Icon name="activity" size={16} />
          </span>
          <span className="monitor-card__person">
            <strong>{label}</strong>
            <span>
              {agent ?? "Open shell"}
              {model ? ` · ${model}` : ""}
            </span>
          </span>
        </button>
        <div className="monitor-card__state">
          <span className={`activity-badge activity-badge--${state}`}>
            {activityLabel(state)}
          </span>
          <span className={`health-label health-label--${health.level}`}>
            {health.label}
          </span>
        </div>
      </header>
      <div className="monitor-card__metrics" aria-label={`Health metrics for ${label}`}>
        <EngineerMetric
          label="Context left"
          value={context ? `${context.remainingPercent}%` : "—"}
          detail={
            context
              ? `${formatTokens(context.usedTokens)} of ${formatTokens(context.windowTokens)}`
              : "Not reported"
          }
          tone={context?.level ?? "unknown"}
          progress={context?.remainingPercent}
        />
        <EngineerMetric
          label="Token spend"
          value={totalTokens == null ? "—" : formatTokens(totalTokens)}
          detail={burnRate == null ? "Not reported" : `${formatTokens(burnRate)}/hr avg`}
          tone="default"
        />
        <EngineerMetric
          label="Uptime"
          value={uptime}
          detail={session.agent_runtime?.state === "running" ? "Agent runtime" : "Terminal open"}
          tone="default"
        />
      </div>
      <div className="monitor-card__activity">
        <span>Current status</span>
        <p>{activitySummary}</p>
      </div>
      <div className="monitor-card__assignment">
        {session.current_plan ? (
          <button
            type="button"
            className="monitor-card__plan"
            onClick={openPlan}
          >
            <span>
              Plan · <strong>{session.current_plan.title}</strong>
            </span>
            <span>
              {session.current_plan.current_phase_title ?? "no current phase"}
              {" · "}
              {session.current_plan.completed_phases}/{session.current_plan.total_phases}
            </span>
          </button>
        ) : (
          <div className="monitor-card__unassigned">
            <span>Assignment</span>
            <strong>{prompt || "No published plan"}</strong>
          </div>
        )}
      </div>
      <div className="monitor-card__report">
        <span className="monitor-card__section-label">Latest report</span>
        <div className="monitor-card__assistant">
          {assistant ? (
            <Markdown source={assistant} compact />
          ) : (
            <span className="monitor-card__muted">
              {item?.turn
                ? "No assistant text after current filters."
                : "Waiting for transcript data."}
            </span>
          )}
        </div>
      </div>
      <footer className="monitor-card__footer">
        <span>
          {item?.turn ? `Reported ${relativeAge(item.turn.end_timestamp)}` : "No reports yet"}
        </span>
        <span>
          {item?.total_event_count ?? 0} events
          {item?.turn && item.turn.operation_count > 0
            ? ` · ${item.turn.operation_count} tools`
            : ""}
          {item?.turn?.has_errors ? " · errors" : ""}
        </span>
      </footer>
    </article>
  );
}

function EngineerMetric({
  label,
  value,
  detail,
  tone,
  progress,
}: {
  label: string;
  value: string;
  detail: string;
  tone: "default" | "healthy" | "warning" | "critical" | "unknown";
  progress?: number;
}) {
  return (
    <div className={`engineer-metric engineer-metric--${tone}`}>
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
      {progress == null ? null : (
        <progress max={100} value={progress} aria-label={`${label}: ${value}`} />
      )}
    </div>
  );
}

function TeamPlans({ plans }: { plans: PlanSummaryView[] }) {
  const openTab = useTabs((store) => store.openTab);
  const ordered = useMemo(
    () =>
      [...plans].sort(
        (a, b) =>
          Number(b.blocked_phases > 0) - Number(a.blocked_phases > 0) ||
          new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime(),
      ),
    [plans],
  );
  return (
    <div className="monitor-team__plans" aria-label="Team plans">
      {ordered.map((plan) => (
        <OpenPlanButton key={plan.id} plan={plan} onOpen={openTab} />
      ))}
    </div>
  );
}

function OpenPlanButton({
  plan,
  onOpen,
}: {
  plan: PlanSummaryView;
  onOpen: TabStore["openTab"];
}) {
  const open = useCallback(
    () => onOpen({ kind: "plan", repo: plan.repo_name, planId: plan.id }),
    [onOpen, plan.id, plan.repo_name],
  );
  return (
    <button
      type="button"
      className={plan.blocked_phases > 0 ? "is-blocked" : undefined}
      onClick={open}
    >
      <span>
        <strong>{plan.title}</strong>
        <small>{plan.current_phase_title ?? "No current phase"}</small>
      </span>
      <span>
        {plan.completed_phases}/{plan.total_phases}
      </span>
    </button>
  );
}

const ACTIVITY_STATES: SessionActivityState[] = [
  "needs_input",
  "blocked",
  "working",
  "awaiting_prompt",
  "starting",
  "shell",
  "unknown",
];

function operationalState(session: SessionView): SessionActivityState {
  if (session.agent_runtime?.state === "starting") return "starting";
  if (session.agent_runtime?.state !== "running") return "shell";
  return session.activity?.state ?? "unknown";
}

function activityCounts(sessions: SessionView[]) {
  const byState = Object.fromEntries(
    ACTIVITY_STATES.map((state) => [state, 0]),
  ) as Record<SessionActivityState, number>;
  for (const session of sessions) byState[operationalState(session)] += 1;
  return {
    byState,
    working: byState.working + byState.starting,
    attention: byState.needs_input + byState.blocked,
  };
}

function sessionActivityCompare(a: SessionView, b: SessionView): number {
  const priority: Record<SessionActivityState, number> = {
    needs_input: 0,
    blocked: 1,
    working: 2,
    starting: 3,
    awaiting_prompt: 4,
    unknown: 5,
    shell: 6,
  };
  const stateOrder =
    priority[operationalState(a)] - priority[operationalState(b)];
  if (stateOrder !== 0) return stateOrder;
  return a.repo.localeCompare(b.repo) || a.created_at.localeCompare(b.created_at);
}

function buildTeams(
  sessions: SessionView[],
  plans: PlanSummaryView[],
): MonitorTeam[] {
  const teams = new Map<string, MonitorTeam>();
  for (const session of sessions) {
    const team = teams.get(session.repo) ?? {
      repo: session.repo,
      sessions: [],
      plans: [],
    };
    team.sessions.push(session);
    teams.set(session.repo, team);
  }
  for (const plan of plans) {
    const team = teams.get(plan.repo_name) ?? {
      repo: plan.repo_name,
      sessions: [],
      plans: [],
    };
    team.plans.push(plan);
    teams.set(plan.repo_name, team);
  }
  return Array.from(teams.values()).sort(
    (a, b) =>
      activityCounts(b.sessions).attention -
        activityCounts(a.sessions).attention || a.repo.localeCompare(b.repo),
  );
}

type HealthLevel = "healthy" | "warning" | "critical" | "unknown";

interface ContextHealth {
  usedTokens: number;
  windowTokens: number;
  remainingPercent: number;
  level: Exclude<HealthLevel, "unknown">;
}

function contextHealth(session: SessionView): ContextHealth | null {
  const usedTokens = session.agent_usage?.context_tokens;
  const windowTokens =
    session.agent_usage?.model_context_window ??
    session.agent_metadata?.model_context_window;
  if (
    usedTokens == null ||
    windowTokens == null ||
    usedTokens < 0 ||
    windowTokens <= 0
  ) {
    return null;
  }
  const remainingPercent = Math.floor(
    Math.max(0, Math.min(100, 100 - (usedTokens / windowTokens) * 100)),
  );
  const level =
    remainingPercent <= 10
      ? "critical"
      : remainingPercent <= 25
        ? "warning"
        : "healthy";
  return { usedTokens, windowTokens, remainingPercent, level };
}

function engineerHealth(
  session: SessionView,
  context: ContextHealth | null,
): { level: HealthLevel; label: string } {
  const state = operationalState(session);
  if (state === "needs_input" || state === "blocked") {
    return { level: "critical", label: "Needs attention" };
  }
  if (context?.level === "critical") {
    return { level: "critical", label: "Context critical" };
  }
  if (context?.level === "warning") {
    return { level: "warning", label: "Context low" };
  }
  if (state === "working" || state === "starting") {
    return { level: "healthy", label: "Healthy" };
  }
  if (state === "awaiting_prompt") {
    return { level: "healthy", label: "Available" };
  }
  if (state === "shell") {
    return { level: "unknown", label: "No agent" };
  }
  return { level: "unknown", label: "Signal pending" };
}

function activityLabel(state: SessionActivityState): string {
  switch (state) {
    case "needs_input":
      return "Needs input";
    case "awaiting_prompt":
      return "Awaiting prompt";
    default:
      return state.charAt(0).toUpperCase() + state.slice(1);
  }
}

function defaultActivitySummary(state: SessionActivityState): string {
  switch (state) {
    case "needs_input":
      return "Agent is waiting for a decision or permission";
    case "blocked":
      return "Agent reported a blocker";
    case "working":
      return "Agent turn is in progress";
    case "awaiting_prompt":
      return "Agent finished its turn";
    case "starting":
      return "Agent process is starting";
    case "shell":
      return "Terminal is open without a running agent";
    case "unknown":
      return "Agent is running; activity has not been reported";
  }
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

const COMPACT_NUMBER = new Intl.NumberFormat("en", {
  notation: "compact",
  maximumFractionDigits: 1,
});

function formatTokens(tokens: number): string {
  return COMPACT_NUMBER.format(Math.max(0, Math.round(tokens)));
}

function averageTokensPerHour(
  totalTokens: number | null,
  startedAt: string,
): number | null {
  if (totalTokens == null) return null;
  const elapsedMs = Date.now() - new Date(startedAt).getTime();
  if (!Number.isFinite(elapsedMs) || elapsedMs < 60_000) return null;
  return totalTokens / (elapsedMs / 3_600_000);
}

function elapsedDuration(iso: string): string {
  const elapsedMs = Math.max(0, Date.now() - new Date(iso).getTime());
  if (!Number.isFinite(elapsedMs)) return "—";
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes < 1) return "<1m";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return `${hours}h ${minutes % 60}m`;
  return `${Math.floor(hours / 24)}d`;
}

function relativeAge(iso: string): string {
  const elapsedMs = Math.max(0, Date.now() - new Date(iso).getTime());
  if (!Number.isFinite(elapsedMs)) return iso;
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}
