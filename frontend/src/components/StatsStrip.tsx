// Compact resource panel pinned to the bottom of the sidebar. Polls
// /api/stats every 10s. Collapsed by default — just a one-line
// "mem/cpu/sessions" summary. Click to expand for Postgres and
// ingester details. No history, no alerting; this exists to answer
// "is this deploy sized correctly?" at a glance (ticket #27).

import { useEffect, useState } from "react";

import { getStats } from "../api/client";
import type { StatsResponse } from "../api/types";
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
    <div className="stats-strip">
      <button
        type="button"
        className="stats-strip__summary"
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
        aria-label="Toggle stats details"
        title={`uptime ${formatUptime(s.uptime_seconds)}`}
      >
        <span className="stats-strip__chev">{expanded ? "▾" : "▸"}</span>
        <span className="stats-strip__pill" title="Memory (RSS / cgroup limit)">
          🧠 {memDisplay}
        </span>
        <span className="stats-strip__pill" title="Process CPU percent">
          ⚙ {cpuDisplay}
        </span>
        <span className="stats-strip__pill" title="Tracked PTY sessions">
          ▶ {s.pty.tracked_sessions}
        </span>
      </button>
      {memPct != null && (
        <div
          className="stats-strip__mem-bar"
          aria-label="Memory usage vs. container limit"
        >
          <div
            className="stats-strip__mem-fill"
            // eslint-disable-next-line local/no-inline-styles -- memPct is a computed percentage, not a finite state
            style={{ width: `${memPct}%` }}
            data-danger={memPct > 85 ? "true" : "false"}
          />
        </div>
      )}
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
