import { useCallback, useEffect, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { getMonitorTimeline } from "../api/client";
import type {
  MonitorSessionTurn,
  MonitorTimelineRequest,
  MonitorTimelineResponse,
  PlanSummaryView,
  RepoGitSummary,
  SessionActivityState,
  SessionView,
  TimelineTurn,
} from "../api/types";
import type { IconName } from "../icons";
import { isResumableSession, useResumeSession } from "../hooks/useResumeSession";
import { appCommands } from "../state/AppCommands";
import { useSessions } from "../state/SessionStore";
import { useTabs } from "../state/TabStore";
import {
  contextMenuTriggerProps,
  type MenuItem,
  useContextMenu,
} from "./common/contextMenuStore";
import { Markdown } from "./timeline/Markdown";
import { Sigil, Tooltip } from "./ui";
import "./MonitorPane.css";

export function MonitorPane({
  active = true,
  onNavigate,
}: {
  active?: boolean;
  /** Called after any action that navigates to another surface (open
   * terminal / timeline / plan / diff). The modal host passes its close
   * handler so navigation from the overlay dismisses it. */
  onNavigate?: () => void;
}) {
  const [data, setData] = useState<MonitorTimelineResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const { sessions, plans, repos } = useSessions(
    useShallow((store) => ({
      sessions: store.sessions,
      plans: store.plans,
      repos: store.repos,
    })),
  );
  const openTab = useTabs((store) => store.openTab);
  const liveSessions = useMemo(
    () =>
      sessions
        .filter((session) => session.state === "live")
        .sort(sessionActivityCompare),
    [sessions],
  );
  // Abandoned: the PTY is gone but the agent transcript survives, so one
  // click can resume it into a fresh terminal.
  const abandonedSessions = useMemo(
    () =>
      sessions
        .filter(isResumableSession)
        .sort((a, b) =>
          (b.last_event_at ?? b.created_at).localeCompare(
            a.last_event_at ?? a.created_at,
          ),
        ),
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

  // The monitor is a fixed overview surface: it always asks for the
  // unfiltered latest turn per session (timeline filter chips stay on the
  // timeline pane).
  const request = useMemo<MonitorTimelineRequest>(
    () => ({
      session_ids: sessionIds,
      hidden_speakers: [],
      hidden_operation_categories: [],
      errors_only: false,
      show_bookkeeping: false,
      show_sidechain: false,
      file_path: undefined,
    }),
    [sessionIds],
  );

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
  }, [active, request, sessionIds.length, sessionRevisionKey]);

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
    () => buildTeams(liveSessions, abandonedSessions, plans),
    [abandonedSessions, liveSessions, plans],
  );
  const contextWatchCount = useMemo(
    () =>
      liveSessions.filter((session) => {
        const context = contextHealth(session);
        return context?.level === "warning" || context?.level === "critical";
      }).length,
    [liveSessions],
  );
  const gitByRepo = useMemo(() => {
    const map = new Map<string, RepoGitSummary>();
    for (const repo of repos) {
      if (repo.git) map.set(repo.name, repo.git);
    }
    return map;
  }, [repos]);
  // Live-session token rollup: fresh work vs full processed volume (the
  // difference is cache reads re-counting the context each API call).
  const tokenTotals = useMemo(() => {
    let fresh = 0;
    let processed = 0;
    for (const session of liveSessions) {
      const usage = session.agent_usage;
      if (!usage) continue;
      processed += usage.total_tokens;
      fresh += Math.max(usage.total_tokens - usage.cached_input_tokens, 0);
    }
    return { fresh, processed };
  }, [liveSessions]);
  const openMetrics = useCallback(() => {
    openTab({ kind: "metrics" }, "top");
    onNavigate?.();
  }, [onNavigate, openTab]);

  return (
    <div className="monitor-pane" data-testid="monitor-pane">
      <header className="monitor-pane__bar">
        <h2>Team overview</h2>
        <div
          className="monitor-pane__totals"
          aria-label="Portfolio health summary"
        >
          <BarStat value={teams.length} label="teams" />
          <BarStat value={liveSessions.length} label="terminals" />
          <BarStat value={counts.working} label="working" tone="ok" />
          <BarStat
            value={counts.attention}
            label="need input"
            tone={counts.attention > 0 ? "atn" : "mute"}
          />
          <BarStat
            value={contextWatchCount}
            label="ctx low"
            tone={contextWatchCount > 0 ? "warn" : "mute"}
          />
          <BarStat
            value={abandonedSessions.length}
            label="resumable"
            tone="mute"
          />
          <Tooltip
            label={`Live sessions: ${formatTokens(tokenTotals.fresh)} fresh tokens of ${formatTokens(tokenTotals.processed)} processed (the rest is cache reads re-counting context)`}
          >
            <span className="monitor-bar-stat monitor-bar-stat--tokens">
              <strong className="tabular">
                {formatTokens(tokenTotals.fresh)}
              </strong>{" "}
              fresh ·{" "}
              <span className="tabular">
                {formatTokens(tokenTotals.processed)}
              </span>{" "}
              processed
            </span>
          </Tooltip>
        </div>
        <button
          type="button"
          className="monitor-pane__metrics-link"
          onClick={openMetrics}
        >
          metrics
        </button>
        <span className="monitor-pane__refresh">
          {loading ? "refreshing" : "live telemetry"}
        </span>
      </header>
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
              git={gitByRepo.get(team.repo) ?? null}
              onNavigate={onNavigate}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function BarStat({
  value,
  label,
  tone = "default",
}: {
  value: number;
  label: string;
  tone?: "default" | "ok" | "warn" | "atn" | "mute";
}) {
  return (
    <span className={`monitor-bar-stat monitor-bar-stat--${tone}`}>
      <strong className="tabular">{value}</strong> {label}
    </span>
  );
}

interface MonitorTeam {
  repo: string;
  sessions: SessionView[];
  /** Orphaned-but-resumable sessions, newest activity first. */
  abandoned: SessionView[];
  plans: PlanSummaryView[];
}

function TeamSection({
  team,
  turnBySession,
  git,
  onNavigate,
}: {
  team: MonitorTeam;
  turnBySession: Map<string, MonitorSessionTurn>;
  git: RepoGitSummary | null;
  onNavigate?: () => void;
}) {
  const openTab = useTabs((store) => store.openTab);
  const openCtx = useContextMenu((store) => store.open);
  const headerMenuTrigger = useMemo(
    () =>
      contextMenuTriggerProps(openCtx, () => [
        {
          kind: "item" as const,
          id: "open-repo-plans",
          label: "Open published plans",
          onSelect: () => {
            appCommands.openPlan({ repo: team.repo });
            onNavigate?.();
          },
        },
        {
          kind: "item" as const,
          id: "open-repo-timeline",
          label: "Open repo timeline",
          onSelect: () => {
            openTab({ kind: "timeline", repo: team.repo }, "bottom");
            onNavigate?.();
          },
        },
        {
          kind: "item" as const,
          id: "open-repo-diff",
          label: "Open repo diff",
          onSelect: () => {
            openTab({ kind: "diff", repo: team.repo });
            onNavigate?.();
          },
        },
        { kind: "separator" as const },
        {
          kind: "item" as const,
          id: "reveal-repo",
          label: "Reveal in sidebar",
          onSelect: () => {
            appCommands.revealRepo({ repo: team.repo });
            onNavigate?.();
          },
        },
      ]),
    [onNavigate, openCtx, openTab, team.repo],
  );
  const counts = activityCounts(team.sessions);
  // Plans already visible on a staffed row stay off the header to keep it
  // one line; the header only carries unstaffed (or unattached) plans.
  const staffedPlanIds = new Set(
    team.sessions
      .map((session) => session.current_plan?.id)
      .filter((id): id is string => id != null),
  );
  const headerPlans = team.plans.filter((plan) => !staffedPlanIds.has(plan.id));
  return (
    <section className="monitor-team" aria-label={`${team.repo} team`}>
      {/* The mark carries the a11y-complete menu trigger (keyboard +
        * right-click); the header-wide listener is a pointer-only
        * convenience so right-click works anywhere on the row. */}
      {/* eslint-disable-next-line jsx-a11y/no-static-element-interactions -- pointer-only augmentation; the keyboard path lives on the mark's trigger */}
      <header
        className="monitor-team__header"
        onContextMenu={headerMenuTrigger.onContextMenu}
      >
        <span
          className="monitor-team__mark"
          aria-label={`${team.repo} repo actions`}
          {...headerMenuTrigger}
        >
          {team.repo.slice(0, 2).toUpperCase()}
        </span>
        <h3>{team.repo}</h3>
        {git ? (
          <Tooltip
            label={[
              git.last_commit
                ? `${git.last_commit.subject}\ncommitted ${relativeAge(git.last_commit.committed_at)}`
                : "No commits reported",
              git.uncommitted_count > 0
                ? `${git.uncommitted_count} uncommitted change${git.uncommitted_count === 1 ? "" : "s"}`
                : null,
            ]
              .filter(Boolean)
              .join("\n")}
          >
            <span className="monitor-team__git tabular">
              <Sigil icon="git-branch" size={12} tone="mute" />
              {git.branch ?? "—"}
              {git.last_commit
                ? ` · ${shortAge(git.last_commit.committed_at)}`
                : ""}
              {git.uncommitted_count > 0 ? (
                <span className="monitor-team__dirty">
                  ±{git.uncommitted_count}
                </span>
              ) : null}
            </span>
          </Tooltip>
        ) : null}
        <span className="monitor-team__meta">
          <Sigil icon="terminal" size={12} tone="mute" />
          <span className="tabular">{team.sessions.length}</span>
        </span>
        {counts.attention > 0 ? (
          <span className="monitor-team__attention">
            {counts.attention} need input
          </span>
        ) : null}
        <span className="monitor-team__spacer" />
        <TeamPlanChips plans={headerPlans} onNavigate={onNavigate} />
      </header>
      {team.sessions.length === 0 && team.abandoned.length === 0 ? (
        <div className="monitor-team__unassigned">No terminal staffed</div>
      ) : (
        <div className="monitor-team__cards">
          {team.sessions.map((session) => (
            <TerminalCard
              key={session.id}
              session={session}
              item={turnBySession.get(session.id) ?? null}
              onNavigate={onNavigate}
            />
          ))}
          {team.abandoned.slice(0, 3).map((session) => (
            <AbandonedCard
              key={session.id}
              session={session}
              onNavigate={onNavigate}
            />
          ))}
          {team.abandoned.length > 3 ? (
            <Tooltip
              label={`${team.abandoned.length - 3} more orphaned session${
                team.abandoned.length - 3 === 1 ? "" : "s"
              } can be resumed from the sidebar`}
            >
              <span className="monitor-team__more-abandoned">
                +{team.abandoned.length - 3} resumable
              </span>
            </Tooltip>
          ) : null}
        </div>
      )}
    </section>
  );
}

/** An orphaned session with a resumable agent transcript: the PTY is
 * gone (e.g. backend restart) but one click respawns it. Mirrors the
 * SessionEndedPane resume flow exactly. */
function AbandonedCard({
  session,
  onNavigate,
}: {
  session: SessionView;
  onNavigate?: () => void;
}) {
  const openTab = useTabs((store) => store.openTab);
  const deleteSession = useSessions((store) => store.deleteSession);
  const openCtx = useContextMenu((store) => store.open);
  const resumeSession = useResumeSession();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const label = session.label?.trim() || session.id.slice(0, 8);
  const agent = session.current_session_agent ?? "agent";
  const lastActive = session.last_event_at ?? session.created_at;

  const resume = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      await resumeSession(session);
      onNavigate?.();
    } catch (err) {
      setError(err instanceof Error ? err.message : "resume failed");
    } finally {
      setBusy(false);
    }
  }, [onNavigate, resumeSession, session]);
  const openTimeline = useCallback(() => {
    openTab({ kind: "timeline", sessionId: session.id }, "bottom");
    onNavigate?.();
  }, [onNavigate, openTab, session.id]);
  const cardMenuTrigger = useMemo(
    () =>
      contextMenuTriggerProps(openCtx, () => [
        {
          kind: "item" as const,
          id: "resume",
          label: "Resume with new PTY",
          onSelect: () => void resume(),
        },
        {
          kind: "item" as const,
          id: "open-timeline",
          label: "Open timeline",
          onSelect: openTimeline,
        },
        { kind: "separator" as const },
        {
          kind: "item" as const,
          id: "delete",
          label: "Delete session",
          destructive: true,
          onSelect: () => void deleteSession(session.id),
        },
      ]),
    [deleteSession, openCtx, openTimeline, resume, session.id],
  );

  return (
    /* eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions -- pointer-only convenience; the identity button carries the keyboard path to the same menu */
    <article
      className="monitor-card monitor-card--abandoned"
      onContextMenu={cardMenuTrigger.onContextMenu}
    >
      <div className="monitor-card__head">
        <Tooltip label="Orphaned — the PTY is gone but the agent transcript survives">
          <span className="monitor-card__state">
            <Sigil icon="session-orphan" size={12} tone="mute" />
          </span>
        </Tooltip>
        <Tooltip
          label={`${agent} session ${session.current_session_uuid?.slice(0, 8) ?? ""}\nClick to open the timeline`}
        >
          <button
            type="button"
            className="monitor-card__identity"
            onClick={openTimeline}
            onContextMenu={cardMenuTrigger.onContextMenu}
            onKeyDown={cardMenuTrigger.onKeyDown}
            aria-label={`Open timeline for ${label}`}
          >
            {label}
          </button>
        </Tooltip>
        <span className="monitor-card__report">
          <span className="tabular">{shortAge(lastActive)}</span>
        </span>
      </div>
      <p className="monitor-card__summary monitor-card__summary--shell">
        {error ?? `orphaned ${agent} session · resumable`}
      </p>
      <div className="monitor-card__stats">
        <button
          type="button"
          className="monitor-card__resume"
          onClick={resume}
          disabled={busy}
          aria-label={`Resume ${label} with a new PTY`}
        >
          <Sigil icon="refresh-cw" size={12} />
          {busy ? "resuming…" : "resume"}
        </button>
      </div>
    </article>
  );
}

function TerminalCard({
  session,
  item,
  onNavigate,
}: {
  session: SessionView;
  item: MonitorSessionTurn | null;
  onNavigate?: () => void;
}) {
  const openTab = useTabs((store) => store.openTab);
  const selectSession = useSessions((store) => store.selectSession);
  const openCtx = useContextMenu((store) => store.open);
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
  const uptimeStart = session.agent_runtime?.started_at ?? session.created_at;
  const uptime = elapsedDuration(uptimeStart);
  const totalTokens = session.agent_usage?.total_tokens ?? null;
  const cachedTokens = session.agent_usage?.cached_input_tokens ?? 0;
  const freshTokens =
    totalTokens == null ? null : Math.max(totalTokens - cachedTokens, 0);
  const burnRate = averageTokensPerHour(freshTokens, uptimeStart);
  const agent = session.current_session_agent ?? session.agent_runtime?.agent;
  const model = session.agent_metadata?.model;
  const openTimeline = useCallback(() => {
    if (!item?.turn) {
      openTab({ kind: "timeline", sessionId: session.id }, "bottom");
    } else {
      openTab(
        {
          kind: "timeline",
          sessionId: session.id,
          focusTurnId: item.turn.id,
          focusKey: crypto.randomUUID(),
        },
        "bottom",
      );
    }
    onNavigate?.();
  }, [item?.turn, onNavigate, openTab, session.id]);
  const openPlan = useCallback(() => {
    if (!session.current_plan) return;
    appCommands.openPlan({
      repo: session.repo,
      planId: session.current_plan.id,
    });
    onNavigate?.();
  }, [onNavigate, session.current_plan, session.repo]);
  const goToTerminal = useCallback(() => {
    selectSession(session.id);
    openTab({ kind: "terminal", sessionId: session.id }, "top");
    openTab({ kind: "timeline", sessionId: session.id }, "bottom");
    onNavigate?.();
  }, [onNavigate, openTab, selectSession, session.id]);
  const cardMenuTrigger = useMemo(
    () =>
      contextMenuTriggerProps(openCtx, () => {
        const items: MenuItem[] = [
          {
            kind: "item",
            id: "go-to-terminal",
            label: "Go to terminal",
            onSelect: goToTerminal,
          },
          {
            kind: "item",
            id: "open-timeline",
            label: "Open timeline",
            onSelect: openTimeline,
          },
          {
            kind: "item",
            id: "future-prompts",
            label: "Future prompts",
            onSelect: () => {
              appCommands.openFuturePrompts({ sessionId: session.id });
              onNavigate?.();
            },
          },
          { kind: "separator" },
        ];
        if (session.current_plan) {
          items.push({
            kind: "item",
            id: "open-plan",
            label: "Open plan",
            onSelect: openPlan,
          });
        }
        items.push(
          {
            kind: "item",
            id: "open-diff",
            label: session.workspace ? "Open workspace diff" : "Open repo diff",
            onSelect: () => {
              openTab({
                kind: "diff",
                repo: session.repo,
                workspaceId: session.workspace?.id,
              });
              onNavigate?.();
            },
          },
          {
            kind: "item",
            id: "open-secrets",
            label: "Open secrets",
            onSelect: () => {
              openTab({ kind: "secrets", sessionId: session.id }, "top");
              onNavigate?.();
            },
          },
        );
        return items;
      }),
    [goToTerminal, onNavigate, openCtx, openPlan, openTab, openTimeline, session],
  );

  return (
    /* eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions -- pointer-only convenience so right-click works anywhere on the card; the identity button carries the keyboard path to the same menu */
    <article
      className={`monitor-card monitor-card--${state}`}
      onContextMenu={cardMenuTrigger.onContextMenu}
    >
      <div className="monitor-card__head">
        <Tooltip label={`${activityLabel(state)} · up ${uptime}`}>
          <span className="monitor-card__state">
            <Sigil icon={stateIcon(state)} size={12} tone={stateTone(state)} />
          </span>
        </Tooltip>
        <Tooltip
          label={[
            [agent, model].filter(Boolean).join(" · ") || "Open shell",
            `up ${uptime}`,
            "Click to open the timeline",
          ].join("\n")}
        >
          <button
            type="button"
            className="monitor-card__identity"
            onClick={openTimeline}
            onContextMenu={cardMenuTrigger.onContextMenu}
            onKeyDown={cardMenuTrigger.onKeyDown}
            aria-label={`Open timeline for ${label}`}
          >
            {label}
          </button>
        </Tooltip>
        <Tooltip
          label={
            item?.turn ? (
              <div className="monitor-report-tip">
                <div className="monitor-report-tip__body">
                  {assistant ? (
                    <Markdown source={assistant} compact />
                  ) : (
                    <em>No assistant output in the latest turn.</em>
                  )}
                </div>
                <div className="monitor-report-tip__meta">
                  reported {relativeAge(item.turn.end_timestamp)} ·{" "}
                  {item.total_event_count} events
                  {item.turn.operation_count > 0
                    ? ` · ${item.turn.operation_count} tools`
                    : ""}
                  {item.turn.has_errors ? " · errors" : ""}
                </div>
              </div>
            ) : (
              "Waiting for transcript data"
            )
          }
        >
          <span
            className="monitor-card__report"
            data-testid={`monitor-report-${session.id}`}
          >
            {item?.turn?.has_errors ? (
              <Sigil icon="alert-triangle" size={12} tone="crit" />
            ) : (
              <Sigil icon="file-text" size={12} tone="mute" />
            )}
            <span className="tabular">
              {item?.turn ? shortAge(item.turn.end_timestamp) : "—"}
            </span>
          </span>
        </Tooltip>
      </div>
      <Tooltip label={activitySummary}>
        <p className={`monitor-card__summary monitor-card__summary--${state}`}>
          {activitySummary}
        </p>
      </Tooltip>
      <div className="monitor-card__stats">
        <Tooltip
          label={
            context
              ? `Context: ${formatTokens(context.usedTokens)} of ${formatTokens(context.windowTokens)} used · ${context.remainingPercent}% left`
              : "Context use not reported"
          }
        >
          <span
            className={`monitor-card__stat monitor-card__ctx monitor-card__ctx--${context?.level ?? "unknown"}`}
            data-testid={`monitor-ctx-${session.id}`}
          >
            {context ? (
              <>
                <span className="tabular">{context.remainingPercent}%</span>
                <span className="monitor-card__ctx-track" aria-hidden="true">
                  <span
                    className="monitor-card__ctx-fill"
                    data-remaining={ctxFillStep(context.remainingPercent)}
                  />
                </span>
              </>
            ) : (
              "—"
            )}
          </span>
        </Tooltip>
        <Tooltip
          label={
            totalTokens == null || freshTokens == null
              ? "Token spend not reported"
              : [
                  `${formatTokens(freshTokens)} fresh`,
                  `${formatTokens(cachedTokens)} cache reads`,
                  `${formatTokens(totalTokens)} processed`,
                  burnRate == null
                    ? null
                    : `${formatTokens(burnRate)} fresh/hr avg`,
                ]
                  .filter(Boolean)
                  .join(" · ")
          }
        >
          <span className="monitor-card__stat tabular">
            {freshTokens == null ? "—" : formatTokens(freshTokens)}
          </span>
        </Tooltip>
        {session.current_plan ? (
          <Tooltip
            label={`${session.current_plan.title}\n${
              session.current_plan.current_phase_title ?? "No current phase"
            } · ${session.current_plan.completed_phases}/${session.current_plan.total_phases} phases`}
          >
            <button
              type="button"
              className={`monitor-card__plan${
                session.current_plan.current_phase_status === "blocked"
                  ? " is-blocked"
                  : ""
              }`}
              onClick={openPlan}
              aria-label={`Plan · ${session.current_plan.title}`}
            >
              <Sigil icon="list-checks" size={12} />
              <span className="tabular">
                {session.current_plan.completed_phases}/
                {session.current_plan.total_phases}
              </span>
            </button>
          </Tooltip>
        ) : (
          <Tooltip
            label={prompt ? `Last prompt:\n${prompt}` : "No published plan"}
          >
            <span className="monitor-card__plan monitor-card__plan--none">
              —
            </span>
          </Tooltip>
        )}
      </div>
    </article>
  );
}

function TeamPlanChips({
  plans,
  onNavigate,
}: {
  plans: PlanSummaryView[];
  onNavigate?: () => void;
}) {
  const ordered = useMemo(
    () =>
      [...plans].sort(
        (a, b) =>
          Number(b.blocked_phases > 0) - Number(a.blocked_phases > 0) ||
          new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime(),
      ),
    [plans],
  );
  if (ordered.length === 0) return null;
  return (
    <span className="monitor-team__plans" aria-label="Unstaffed team plans">
      {ordered.map((plan) => (
        <PlanChip key={plan.id} plan={plan} onNavigate={onNavigate} />
      ))}
    </span>
  );
}

function PlanChip({
  plan,
  onNavigate,
}: {
  plan: PlanSummaryView;
  onNavigate?: () => void;
}) {
  const open = useCallback(() => {
    appCommands.openPlan({ repo: plan.repo_name, planId: plan.id });
    onNavigate?.();
  }, [onNavigate, plan.id, plan.repo_name]);
  return (
    <Tooltip
      label={[
        plan.title,
        [
          `${plan.current_phase_title ?? "No current phase"} · ${plan.completed_phases}/${plan.total_phases} phases`,
          plan.blocked_phases > 0 ? `${plan.blocked_phases} blocked` : null,
        ]
          .filter(Boolean)
          .join(" · "),
      ].join("\n")}
    >
      <button
        type="button"
        className={`monitor-plan-chip${plan.blocked_phases > 0 ? " is-blocked" : ""}`}
        onClick={open}
        aria-label={`Open plan ${plan.title}`}
      >
        <Sigil icon="list-checks" size={12} />
        <span className="tabular">
          {plan.completed_phases}/{plan.total_phases}
        </span>
      </button>
    </Tooltip>
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

function stateIcon(state: SessionActivityState): IconName {
  switch (state) {
    case "needs_input":
      return "alert-triangle";
    case "blocked":
      return "alert-triangle";
    case "working":
      return "activity";
    case "starting":
      return "refresh-cw";
    case "awaiting_prompt":
      return "check";
    case "shell":
      return "terminal";
    case "unknown":
      return "clock";
  }
}

function stateTone(
  state: SessionActivityState,
): "ok" | "crit" | "info" | "atn" | "mute" {
  switch (state) {
    case "needs_input":
      return "atn";
    case "blocked":
      return "crit";
    case "working":
    case "starting":
      return "info";
    case "awaiting_prompt":
      return "ok";
    case "shell":
    case "unknown":
      return "mute";
  }
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
  abandoned: SessionView[],
  plans: PlanSummaryView[],
): MonitorTeam[] {
  const teams = new Map<string, MonitorTeam>();
  const teamFor = (repo: string): MonitorTeam => {
    const team = teams.get(repo) ?? {
      repo,
      sessions: [],
      abandoned: [],
      plans: [],
    };
    teams.set(repo, team);
    return team;
  };
  for (const session of sessions) {
    teamFor(session.repo).sessions.push(session);
  }
  for (const session of abandoned) {
    teamFor(session.repo).abandoned.push(session);
  }
  for (const plan of plans) {
    teamFor(plan.repo_name).plans.push(plan);
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

/** Quantize remaining-context percent to a 0–10 step so the mini gauge can be
 * driven by a finite attribute set instead of an inline width. */
function ctxFillStep(remainingPercent: number): number {
  return Math.max(0, Math.min(10, Math.round(remainingPercent / 10)));
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

function shortAge(iso: string): string {
  const elapsedMs = Math.max(0, Date.now() - new Date(iso).getTime());
  if (!Number.isFinite(elapsedMs)) return "—";
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
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
