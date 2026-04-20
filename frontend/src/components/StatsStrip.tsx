// Compact resource panel pinned to the bottom of the sidebar. Polls
// /api/stats every 10s. Collapsed by default — just a one-line
// "mem/cpu/sessions" summary. Click to expand for Postgres and
// ingester details. No history, no alerting; this exists to answer
// "is this deploy sized correctly?" at a glance (ticket #27).

import { useCallback, useEffect, useMemo, useState } from "react";

import { getStats } from "../api/client";
import type { StatsResponse } from "../api/types";
import { Icon } from "../icons";
import { Tooltip } from "./ui";
import "./StatsStrip.css";

const POLL_MS = 10_000;

export function StatsStrip() {
  const [stats, setStats] = useState<StatsResponse | null>(null);
  const [error, setError] = useState(false);
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const s = await getStats();
        if (!cancelled) {
          setStats(s);
          setError(false);
        }
      } catch {
        if (!cancelled) setError(true);
      }
    };
    void load();
    const timer = setInterval(load, POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);

  const toggleExpanded = useCallback(() => setExpanded((v) => !v), []);

  if (!stats && !error) {
    return (
      <div className="stats-strip stats-strip--loading" aria-live="polite">
        <span>stats…</span>
      </div>
    );
  }

  if (error && !stats) {
    return (
      <div className="stats-strip stats-strip--err" role="status">
        <span>stats unavailable</span>
      </div>
    );
  }

  const s = stats!;
  const memUsedMb = s.process.memory_rss_bytes / (1024 * 1024);
  const memLimitMb = s.process.memory_limit_bytes
    ? s.process.memory_limit_bytes / (1024 * 1024)
    : null;
  const memDisplay = memLimitMb
    ? `${memUsedMb.toFixed(0)} / ${memLimitMb.toFixed(0)} MB`
    : `${memUsedMb.toFixed(0)} MB`;
  const memPct = memLimitMb ? Math.min(100, (memUsedMb / memLimitMb) * 100) : null;
  const cpuDisplay = `${s.process.cpu_percent.toFixed(0)}%`;

  return (
    <div className="stats-strip" data-testid="stats-strip">
      <Tooltip label={`uptime ${formatUptime(s.uptime_seconds)}`}>
        <button
          type="button"
          className="stats-strip__summary"
          onClick={toggleExpanded}
          aria-expanded={expanded}
          aria-label="Toggle stats details"
        >
          <span
            className={
              expanded
                ? "stats-strip__chev stats-strip__chev--open"
                : "stats-strip__chev"
            }
          >
            <Icon name="chevron-right" size={12} />
          </span>
          <Tooltip label="Memory (RSS / cgroup limit)">
            <span className="stats-strip__pill tabular">
              <Icon name="cpu" size={12} />
              {memDisplay}
            </span>
          </Tooltip>
          <Tooltip label="Process CPU percent">
            <span className="stats-strip__pill tabular">
              <Icon name="activity" size={12} />
              {cpuDisplay}
            </span>
          </Tooltip>
          <Tooltip label="Tracked PTY sessions">
            <span className="stats-strip__pill tabular">
              <Icon name="terminal" size={12} />
              {s.pty.tracked_sessions}
            </span>
          </Tooltip>
        </button>
      </Tooltip>
      {memPct != null && <MemBar pct={memPct} />}
      {expanded && (
        <dl className="stats-strip__details">
          <div>
            <dt>uptime</dt>
            <dd>{formatUptime(s.uptime_seconds)}</dd>
          </div>
          <div>
            <dt>db size</dt>
            <dd>{formatBytes(s.db.database_size_bytes)}</dd>
          </div>
          <div>
            <dt>events</dt>
            <dd>{s.db.events_rowcount.toLocaleString()}</dd>
          </div>
          <div>
            <dt>agent sessions</dt>
            <dd>{s.db.agent_sessions_rowcount.toLocaleString()}</dd>
          </div>
          <div>
            <dt>pty sessions</dt>
            <dd>{s.db.pty_sessions_rowcount.toLocaleString()}</dd>
          </div>
          <div>
            <dt>files tracked</dt>
            <dd>{s.db.ingester_state_rowcount.toLocaleString()}</dd>
          </div>
          <div>
            <dt>ingested events</dt>
            <dd>{s.ingester.events_inserted_total.toLocaleString()}</dd>
          </div>
          <div>
            <dt>parse errors</dt>
            <dd>{s.ingester.parse_errors_total.toLocaleString()}</dd>
          </div>
        </dl>
      )}
    </div>
  );
}

function MemBar({ pct }: { pct: number }) {
  const style = useMemo(() => ({ width: `${pct}%` }), [pct]);
  return (
    <div
      className="stats-strip__mem-bar"
      aria-label="Memory usage vs. container limit"
    >
      <div
        className="stats-strip__mem-fill"
        // eslint-disable-next-line local/no-inline-styles -- pct is a computed percentage, not a finite state
        style={style}
        data-danger={pct > 85 ? "true" : "false"}
      />
    </div>
  );
}

function formatUptime(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  const mr = m % 60;
  if (h < 24) return `${h}h ${mr}m`;
  const d = Math.floor(h / 24);
  const hr = h % 24;
  return `${d}d ${hr}h`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb.toFixed(0)} KB`;
  const mb = kb / 1024;
  if (mb < 1024) return `${mb.toFixed(1)} MB`;
  const gb = mb / 1024;
  return `${gb.toFixed(2)} GB`;
}
