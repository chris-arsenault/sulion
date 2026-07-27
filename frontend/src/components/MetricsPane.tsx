import { useCallback, useEffect, useMemo, useState } from "react";

import { getMetrics } from "../api/client";
import type {
  FlowCfdDay,
  MetricsResponse,
  PlanBurndownView,
  RepoGitActivityView,
} from "../api/types";
import { appCommands } from "../state/AppCommands";
import { Sigil, Tooltip } from "./ui";
import "./MetricsPane.css";

const REFRESH_MS = 60_000;

// Chart palette, validated with the dataviz six-checks script against the
// dark chart surface (#0c0e13). CFD band order is bottom→top:
// completed, in_progress, blocked, pending — red and green never adjacent.
const CFD_BANDS = [
  { key: "completed", label: "done", color: "#27a862" },
  { key: "in_progress", label: "in progress", color: "#4a90e2" },
  { key: "blocked", label: "blocked", color: "#e0484f" },
  { key: "pending", label: "pending", color: "#a678e0" },
] as const;
const BAR_COLOR = "#4a90e2";

export function MetricsPane({
  active = true,
  onOpenOverview,
}: {
  active?: boolean;
  /** Modal host override: switch the overlay back to the team overview
   * instead of leaving the modal. */
  onOpenOverview?: () => void;
}) {
  const [data, setData] = useState<MetricsResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setData(await getMetrics());
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "metrics fetch failed");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void load();
    const timer = setInterval(() => void load(), REFRESH_MS);
    return () => clearInterval(timer);
  }, [active, load]);

  return (
    <div className="metrics-pane" data-testid="metrics-pane">
      <header className="metrics-pane__bar">
        <h2>Metrics</h2>
        <span className="metrics-pane__refresh">
          {loading ? "refreshing" : data ? `as of ${timeOf(data.generated_at)}` : ""}
        </span>
        {onOpenOverview ? (
          <button
            type="button"
            className="metrics-pane__overview-link"
            onClick={onOpenOverview}
          >
            overview
          </button>
        ) : null}
      </header>
      {error ? <div className="metrics-pane__error">{error}</div> : null}
      {!data ? (
        <div className="metrics-pane__empty">
          {error ? "Metrics unavailable." : "Loading metrics…"}
        </div>
      ) : (
        <div className="metrics-pane__grid">
          <TokensSection data={data} />
          <FlowSection data={data} />
          <GitSection data={data} />
          <ChurnSection data={data} />
        </div>
      )}
    </div>
  );
}

// ─── Tokens ─────────────────────────────────────────────────────────

function TokensSection({ data }: { data: MetricsResponse }) {
  const { usage } = data;
  const maxDay = Math.max(1, ...usage.daily.map((d) => d.fresh_tokens));
  return (
    <section className="metrics-section" aria-label="Token spend">
      <header>Tokens</header>
      <div className="metrics-tiles">
        <StatTile
          label="today"
          value={formatTokens(usage.today.fresh_tokens)}
          detail={`${formatTokens(usage.today.total_tokens)} processed`}
        />
        <StatTile
          label="7 days"
          value={formatTokens(usage.last_7d.fresh_tokens)}
          detail={`${formatTokens(usage.last_7d.total_tokens)} processed`}
        />
        <StatTile
          label="all time"
          value={formatTokens(usage.all_time.fresh_tokens)}
          detail={`${formatTokens(usage.all_time.total_tokens)} processed`}
        />
      </div>
      <figure className="metrics-chart">
        <figcaption>Fresh tokens per day</figcaption>
        {usage.daily.length === 0 ? (
          <div className="metrics-chart__empty">No usage recorded yet.</div>
        ) : (
          <div className="metrics-bars" role="img" aria-label="Fresh tokens per day">
            {usage.daily.map((d) => (
              <Tooltip
                key={d.day}
                label={`${dayLabel(d.day)}\n${formatTokens(d.fresh_tokens)} fresh · ${formatTokens(d.cached_tokens)} cache reads · ${formatTokens(d.total_tokens)} processed`}
              >
                <div className="metrics-bars__slot">
                  <div
                    className="metrics-bars__fill"
                    data-h={barStep(d.fresh_tokens, maxDay)}
                  />
                </div>
              </Tooltip>
            ))}
          </div>
        )}
        <div className="metrics-chart__xlabels">
          <span>{usage.daily[0] ? dayLabel(usage.daily[0].day) : ""}</span>
          <span>
            {usage.daily.at(-1) ? dayLabel(usage.daily.at(-1)!.day) : ""}
          </span>
        </div>
      </figure>
      <table className="metrics-table" aria-label="Tokens by repo">
        <thead>
          <tr>
            <th>repo</th>
            <th>today</th>
            <th>7d</th>
            <th>all time</th>
          </tr>
        </thead>
        <tbody>
          {usage.per_repo.slice(0, 8).map((row) => (
            <tr key={row.repo}>
              <td>{row.repo}</td>
              <td className="tabular">{formatTokens(row.today.fresh_tokens)}</td>
              <td className="tabular">{formatTokens(row.last_7d.fresh_tokens)}</td>
              <td className="tabular">
                {formatTokens(row.all_time.fresh_tokens)}
                <span className="metrics-table__minor">
                  {" "}
                  / {formatTokens(row.all_time.total_tokens)}
                </span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="metrics-note">
        fresh = input + cache writes + output; processed additionally counts
        cache reads of prior context.
      </p>
    </section>
  );
}

// ─── Flow ───────────────────────────────────────────────────────────

function FlowSection({ data }: { data: MetricsResponse }) {
  const { flow } = data;
  const throughput = flow.throughput_weeks.at(-1)?.completed_weight ?? 0;
  return (
    <section className="metrics-section" aria-label="Delivery flow">
      <header>Delivery flow</header>
      <div className="metrics-tiles">
        <StatTile label="in progress" value={String(flow.wip)} detail="phases now" />
        <StatTile
          label="blocked"
          value={String(flow.blocked)}
          detail="phases now"
          tone={flow.blocked > 0 ? "crit" : undefined}
        />
        <StatTile
          label="throughput"
          value={String(throughput)}
          detail="weight this week"
        />
        <StatTile
          label="cycle time"
          value={
            flow.cycle_time_hours_p50 == null
              ? "—"
              : formatHours(flow.cycle_time_hours_p50)
          }
          detail="median, 30d"
        />
      </div>
      <Cfd cfd={flow.cfd} />
      {flow.burndowns.slice(0, 4).map((burndown) => (
        <Burndown key={burndown.plan_id} burndown={burndown} />
      ))}
    </section>
  );
}

function Cfd({ cfd }: { cfd: FlowCfdDay[] }) {
  const bands = useMemo(() => {
    if (cfd.length === 0) return null;
    // skipped counts as done for the chart; the tooltip breaks it out.
    const totals = cfd.map(
      (d) => d.pending + d.in_progress + d.blocked + d.completed + d.skipped,
    );
    const max = Math.max(1, ...totals);
    const stacked = cfd.map((d) => {
      const done = d.completed + d.skipped;
      return [done, done + d.in_progress, done + d.in_progress + d.blocked, done + d.in_progress + d.blocked + d.pending];
    });
    const x = (i: number) =>
      cfd.length === 1 ? 50 : (i / (cfd.length - 1)) * 100;
    const y = (v: number) => 40 - (v / max) * 40;
    const path = (level: number) => {
      const top = stacked
        .map((s, i) => `${i === 0 ? "M" : "L"}${x(i)},${y(s[level]!)}`)
        .join(" ");
      const base =
        level === 0
          ? `L100,40 L0,40`
          : stacked
              .map(
                (_, i) =>
                  `L${x(cfd.length - 1 - i)},${y(stacked[cfd.length - 1 - i]![level - 1]!)}`,
              )
              .join(" ");
      return `${top} ${base} Z`;
    };
    return { path };
  }, [cfd]);
  if (!bands) {
    return <div className="metrics-chart__empty">No plan history yet.</div>;
  }
  return (
    <figure className="metrics-chart">
      <figcaption>Cumulative flow (phase weight, 14d)</figcaption>
      <div className="metrics-cfd">
        <svg
          viewBox="0 0 100 40"
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          {CFD_BANDS.map((band, level) => (
            <path
              key={band.key}
              d={bands.path(level)}
              fill={band.color}
              stroke="#0c0e13"
              strokeWidth={1}
              vectorEffect="non-scaling-stroke"
            />
          ))}
        </svg>
        <div className="metrics-cfd__hover">
          {cfd.map((d) => (
            <Tooltip
              key={d.day}
              label={[
                dayLabel(d.day),
                [
                  `pending ${d.pending}`,
                  `in progress ${d.in_progress}`,
                  `blocked ${d.blocked}`,
                  `done ${d.completed + d.skipped}`,
                  d.skipped ? `${d.skipped} skipped` : null,
                ]
                  .filter(Boolean)
                  .join(" · "),
              ].join("\n")}
            >
              <span className="metrics-cfd__cell" />
            </Tooltip>
          ))}
        </div>
      </div>
      <div className="metrics-legend" aria-label="Cumulative flow legend">
        {CFD_BANDS.map((band) => (
          <span key={band.key} className="metrics-legend__item">
            <span
              className="metrics-legend__swatch"
              data-band={band.key}
              aria-hidden="true"
            />
            {band.label}
          </span>
        ))}
      </div>
      <div className="metrics-chart__xlabels">
        <span>{cfd[0] ? dayLabel(cfd[0].day) : ""}</span>
        <span>{cfd.at(-1) ? dayLabel(cfd.at(-1)!.day) : ""}</span>
      </div>
    </figure>
  );
}

function Burndown({ burndown }: { burndown: PlanBurndownView }) {
  const geometry = useMemo(() => {
    const days = burndown.days;
    if (days.length === 0) return null;
    const max = Math.max(1, ...days.map((d) => d.total_weight));
    const x = (i: number) =>
      days.length === 1 ? 50 : (i / (days.length - 1)) * 100;
    const y = (v: number) => 40 - (v / max) * 40;
    const line = (pick: (d: (typeof days)[number]) => number) =>
      days.map((d, i) => `${i === 0 ? "M" : "L"}${x(i)},${y(pick(d))}`).join(" ");
    return {
      remaining: line((d) => d.remaining_weight),
      scope: line((d) => d.total_weight),
    };
  }, [burndown.days]);
  if (!geometry) return null;
  const last = burndown.days.at(-1);
  return (
    <figure className="metrics-chart">
      <figcaption>
        Burndown · {burndown.repo} / {burndown.title}
        <span className="metrics-chart__figvalue tabular">
          {last ? `${last.remaining_weight}/${last.total_weight} left` : ""}
        </span>
      </figcaption>
      <div className="metrics-cfd">
        <svg viewBox="0 0 100 40" preserveAspectRatio="none" aria-hidden="true">
          <path
            d={geometry.scope}
            fill="none"
            stroke="#6a707c"
            strokeWidth={1.5}
            strokeDasharray="4 3"
            vectorEffect="non-scaling-stroke"
          />
          <path
            d={geometry.remaining}
            fill="none"
            stroke={BAR_COLOR}
            strokeWidth={2}
            vectorEffect="non-scaling-stroke"
          />
        </svg>
        <div className="metrics-cfd__hover">
          {burndown.days.map((d) => (
            <Tooltip
              key={d.day}
              label={`${dayLabel(d.day)}\n${d.remaining_weight} remaining of ${d.total_weight} scope`}
            >
              <span className="metrics-cfd__cell" />
            </Tooltip>
          ))}
        </div>
      </div>
      <div className="metrics-legend">
        <span className="metrics-legend__item">
          <span className="metrics-legend__swatch" data-band="in_progress" aria-hidden="true" />
          remaining
        </span>
        <span className="metrics-legend__item">
          <span className="metrics-legend__swatch metrics-legend__swatch--dashed" aria-hidden="true" />
          scope
        </span>
      </div>
    </figure>
  );
}

// ─── Git activity ───────────────────────────────────────────────────

function GitSection({ data }: { data: MetricsResponse }) {
  const active = useMemo(
    () =>
      [...data.git]
        .filter((repo) => repo.commits_7d > 0)
        .sort((a, b) => b.commits_7d - a.commits_7d),
    [data.git],
  );
  const daily = useMemo(() => mergeGitDaily(data.git), [data.git]);
  const maxDay = Math.max(1, ...daily.map((d) => d.commits));
  const agentShare = useMemo(() => {
    const agent = active.reduce((sum, repo) => sum + repo.agent_commits_7d, 0);
    const human = active.reduce((sum, repo) => sum + repo.human_commits_7d, 0);
    return { agent, human };
  }, [active]);
  return (
    <section className="metrics-section" aria-label="Git activity">
      <header>Git activity</header>
      <div className="metrics-tiles">
        <StatTile
          label="commits 7d"
          value={String(agentShare.agent + agentShare.human)}
          detail={`${agentShare.agent} agent · ${agentShare.human} human`}
        />
        <StatTile
          label="lines 7d"
          value={`+${formatTokens(active.reduce((sum, repo) => sum + repo.insertions_7d, 0))}`}
          detail={`−${formatTokens(active.reduce((sum, repo) => sum + repo.deletions_7d, 0))} removed`}
        />
      </div>
      <figure className="metrics-chart">
        <figcaption>Commits per day (all repos)</figcaption>
        {daily.length === 0 ? (
          <div className="metrics-chart__empty">No commits in 14 days.</div>
        ) : (
          <div className="metrics-bars" role="img" aria-label="Commits per day">
            {daily.map((d) => (
              <Tooltip
                key={d.day}
                label={`${dayLabel(d.day)}\n${d.commits} commit${d.commits === 1 ? "" : "s"} · +${d.insertions} −${d.deletions}`}
              >
                <div className="metrics-bars__slot">
                  <div
                    className="metrics-bars__fill metrics-bars__fill--commits"
                    data-h={barStep(d.commits, maxDay)}
                  />
                </div>
              </Tooltip>
            ))}
          </div>
        )}
        <div className="metrics-chart__xlabels">
          <span>{daily[0] ? dayLabel(daily[0].day) : ""}</span>
          <span>{daily.at(-1) ? dayLabel(daily.at(-1)!.day) : ""}</span>
        </div>
      </figure>
      <table className="metrics-table" aria-label="Git activity by repo">
        <thead>
          <tr>
            <th>repo</th>
            <th>24h</th>
            <th>7d</th>
            <th>lines 7d</th>
            <th>split</th>
          </tr>
        </thead>
        <tbody>
          {active.slice(0, 10).map((repo) => (
            <tr key={repo.repo}>
              <td>{repo.repo}</td>
              <td className="tabular">{repo.commits_24h}</td>
              <td className="tabular">{repo.commits_7d}</td>
              <td className="tabular">
                +{formatTokens(repo.insertions_7d)} −{formatTokens(repo.deletions_7d)}
              </td>
              <td className="tabular">
                <SplitBar agent={repo.agent_commits_7d} human={repo.human_commits_7d} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="metrics-legend">
        <span className="metrics-legend__item">
          <span className="metrics-legend__swatch" data-band="agent" aria-hidden="true" />
          agent
        </span>
        <span className="metrics-legend__item">
          <span className="metrics-legend__swatch" data-band="human" aria-hidden="true" />
          human
        </span>
      </div>
    </section>
  );
}

function SplitBar({ agent, human }: { agent: number; human: number }) {
  const total = agent + human;
  if (total === 0) return <span className="metrics-table__minor">—</span>;
  return (
    <Tooltip label={`${agent} agent · ${human} human`}>
      <span className="metrics-split">
        <span className="metrics-split__agent" data-h={barStep(agent, total)} />
        <span className="metrics-split__human" data-h={barStep(human, total)} />
      </span>
    </Tooltip>
  );
}

// ─── Churn ──────────────────────────────────────────────────────────

function ChurnSection({ data }: { data: MetricsResponse }) {
  if (data.churn.length === 0) {
    return (
      <section className="metrics-section" aria-label="Churn hotspots">
        <header>Churn hotspots</header>
        <div className="metrics-chart__empty">
          No files re-written in 3+ turns this week.
        </div>
      </section>
    );
  }
  return (
    <section className="metrics-section" aria-label="Churn hotspots">
      <header>Churn hotspots · 7d</header>
      <table className="metrics-table" aria-label="Files repeatedly rewritten">
        <thead>
          <tr>
            <th>file</th>
            <th>turns</th>
            <th>sessions</th>
            <th>last</th>
          </tr>
        </thead>
        <tbody>
          {data.churn.slice(0, 12).map((hotspot) => (
            <ChurnRow
              key={`${hotspot.repo}:${hotspot.path}`}
              repo={hotspot.repo}
              path={hotspot.path}
              turns={hotspot.write_turns}
              sessions={hotspot.sessions}
              last={hotspot.last_write_at}
            />
          ))}
        </tbody>
      </table>
      <p className="metrics-note">
        agent writes to the same file across many turns — a rework signal,
        not proof of a problem.
      </p>
    </section>
  );
}

function ChurnRow({
  repo,
  path,
  turns,
  sessions,
  last,
}: {
  repo: string;
  path: string;
  turns: number;
  sessions: number;
  last: string;
}) {
  const openFile = useCallback(
    () => appCommands.openFile({ repo, path }),
    [repo, path],
  );
  return (
    <tr>
      <td>
        <Tooltip label={`${repo}/${path}\nClick to open the file`}>
          <button type="button" className="metrics-table__link" onClick={openFile}>
            <Sigil icon="file-text" size={12} tone="mute" />
            {repo}/{path}
          </button>
        </Tooltip>
      </td>
      <td className="tabular">{turns}</td>
      <td className="tabular">{sessions}</td>
      <td className="tabular">{shortAge(last)}</td>
    </tr>
  );
}

// ─── Primitives & helpers ───────────────────────────────────────────

function StatTile({
  label,
  value,
  detail,
  tone,
}: {
  label: string;
  value: string;
  detail: string;
  tone?: "crit";
}) {
  return (
    <div
      className={["metrics-tile", tone ? `metrics-tile--${tone}` : null]
        .filter(Boolean)
        .join(" ")}
    >
      <span>{label}</span>
      <strong className="tabular">{value}</strong>
      <small>{detail}</small>
    </div>
  );
}

function mergeGitDaily(repos: RepoGitActivityView[]) {
  const byDay = new Map<
    string,
    { day: string; commits: number; insertions: number; deletions: number }
  >();
  for (const repo of repos) {
    for (const d of repo.daily) {
      const entry = byDay.get(d.day) ?? {
        day: d.day,
        commits: 0,
        insertions: 0,
        deletions: 0,
      };
      entry.commits += d.commits;
      entry.insertions += d.insertions;
      entry.deletions += d.deletions;
      byDay.set(d.day, entry);
    }
  }
  return [...byDay.values()].sort((a, b) => a.day.localeCompare(b.day));
}

/** Quantize a value to a 0–20 step so bar heights come from a finite
 * class set instead of inline styles. */
function barStep(value: number, max: number): number {
  if (max <= 0 || value <= 0) return 0;
  return Math.max(1, Math.min(20, Math.round((value / max) * 20)));
}

const COMPACT_NUMBER = new Intl.NumberFormat("en", {
  notation: "compact",
  maximumFractionDigits: 1,
});

function formatTokens(tokens: number): string {
  return COMPACT_NUMBER.format(Math.max(0, Math.round(tokens)));
}

function formatHours(hours: number): string {
  if (hours < 1) return `${Math.round(hours * 60)}m`;
  if (hours < 48) return `${hours.toFixed(hours < 10 ? 1 : 0)}h`;
  return `${Math.round(hours / 24)}d`;
}

function dayLabel(day: string): string {
  const parsed = new Date(`${day}T00:00:00Z`);
  if (Number.isNaN(parsed.getTime())) return day;
  return parsed.toLocaleDateString("en", {
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });
}

function timeOf(iso: string): string {
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return iso;
  return parsed.toLocaleTimeString("en", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
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
