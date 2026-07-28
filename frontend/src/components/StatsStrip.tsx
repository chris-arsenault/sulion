// Compact resource panel pinned to the bottom of the sidebar. The
// unified app-state poll feeds this; the strip does not own a timer.
// The compact row is deliberately "current
// pressure" only; the expanded view separates current load from
// lifetime inventory so cumulative counts do not masquerade as live
// pressure. No history, no alerting; this exists to answer "is this
// deploy sized correctly?" at a glance (ticket #27).

import { useCallback, useMemo, useState, type MouseEvent } from "react";
import { useShallow } from "zustand/react/shallow";

import { approveNodePairing } from "../api/client";
import { Icon } from "../icons";
import { useSessions } from "../state/SessionStore";
import { Tooltip } from "./ui";
import "./StatsStrip.css";

export function StatsStrip() {
  const { nodes, stats, lastError, refresh } = useSessions(
    useShallow((store) => ({
      nodes: store.nodes,
      stats: store.stats,
      lastError: store.lastError,
      refresh: store.refresh,
    })),
  );
  const [expanded, setExpanded] = useState(false);
  const [approvingNodeId, setApprovingNodeId] = useState<string | null>(null);
  const [approvalError, setApprovalError] = useState<string | null>(null);

  const toggleExpanded = useCallback(() => setExpanded((v) => !v), []);
  const approvePairing = useCallback(
    async (nodeId: string) => {
      setApprovingNodeId(nodeId);
      setApprovalError(null);
      try {
        await approveNodePairing(nodeId);
        await refresh();
      } catch (error) {
        setApprovalError(
          error instanceof Error ? error.message : "Node approval failed",
        );
      } finally {
        setApprovingNodeId(null);
      }
    },
    [refresh],
  );
  const approvePendingNode = useCallback(
    (event: MouseEvent<HTMLButtonElement>) => {
      void approvePairing(event.currentTarget.value);
    },
    [approvePairing],
  );
  const primaryNode =
    nodes.find((node) => node.connection_state === "connected") ??
    nodes.find((node) => node.pending_key_fingerprint != null) ??
    nodes[0] ??
    null;

  if (!stats) {
    return (
      <div className="stats-strip stats-strip--loading" aria-live="polite">
        <span>{lastError ? "stats unavailable" : "stats…"}</span>
      </div>
    );
  }

  const s = stats;
  const memUsedMb = s.process.memory_rss_bytes / (1024 * 1024);
  const memLimitMb = s.process.memory_limit_bytes
    ? s.process.memory_limit_bytes / (1024 * 1024)
    : null;
  const memDisplay = memLimitMb
    ? `${memUsedMb.toFixed(0)} / ${memLimitMb.toFixed(0)} MB`
    : `${memUsedMb.toFixed(0)} MB`;
  const memPct = memLimitMb ? Math.min(100, (memUsedMb / memLimitMb) * 100) : null;
  const cpuDisplay = `${s.process.cpu_percent.toFixed(0)}%`;
  const dbSizeDisplay = formatBytes(s.db.database_size_bytes);
  return (
    <div className="stats-strip" data-testid="stats-strip">
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
        <Tooltip label="Backend memory (RSS / cgroup limit)">
          <span className="stats-strip__pill tabular">
            <Icon name="cpu" size={12} />
            {memDisplay}
          </span>
        </Tooltip>
        <Tooltip label="Backend process CPU percent">
          <span className="stats-strip__pill tabular">
            <Icon name="activity" size={12} />
            {cpuDisplay}
          </span>
        </Tooltip>
        <Tooltip label="Current Postgres database size">
          <span className="stats-strip__pill tabular">
            <Icon name="layers" size={12} />
            {dbSizeDisplay}
          </span>
        </Tooltip>
        <Tooltip label="Live PTYs currently tracked by the backend">
          <span className="stats-strip__pill tabular">
            <Icon name="terminal" size={12} />
            {s.pty.live_sessions}
          </span>
        </Tooltip>
        <Tooltip
          label={
            primaryNode
              ? `${primaryNode.display_name}: ${nodeStatusLabel(primaryNode.connection_state)}`
              : "No development node paired"
          }
        >
          <span
            className="stats-strip__pill stats-strip__node"
            data-state={primaryNode?.connection_state ?? "none"}
          >
            <Icon name="activity" size={12} />
            {primaryNode ? nodeStatusLabel(primaryNode.connection_state) : "no node"}
          </span>
        </Tooltip>
      </button>
      {memPct != null && <MemBar pct={memPct} />}
      {expanded && (
        <div className="stats-strip__sections">
          <section className="stats-strip__section" aria-label="Current stats">
            <h3 className="stats-strip__section-title">Current</h3>
            <dl className="stats-strip__details">
              <div>
                <dt>uptime</dt>
                <dd>{formatUptime(s.uptime_seconds)}</dd>
              </div>
              <div>
                <dt>backend mem</dt>
                <dd>{memDisplay}</dd>
              </div>
              <div>
                <dt>backend cpu</dt>
                <dd>{cpuDisplay}</dd>
              </div>
              <div>
                <dt>db size</dt>
                <dd>{dbSizeDisplay}</dd>
              </div>
              <div>
                <dt>live PTYs</dt>
                <dd>{s.pty.live_sessions.toLocaleString()}</dd>
              </div>
              <div>
                <dt>agent PTYs</dt>
                <dd>{s.pty.live_agent_sessions.toLocaleString()}</dd>
              </div>
            </dl>
          </section>
          <section className="stats-strip__section" aria-label="Inventory stats">
            <h3 className="stats-strip__section-title">Inventory</h3>
            <dl className="stats-strip__details">
              <div>
                <dt>event rows</dt>
                <dd>{s.inventory.event_rows.toLocaleString()}</dd>
              </div>
              <div>
                <dt>agent sessions total</dt>
                <dd>{s.inventory.agent_sessions.toLocaleString()}</dd>
              </div>
              <div>
                <dt>pty sessions total</dt>
                <dd>{s.inventory.pty_sessions.toLocaleString()}</dd>
              </div>
              <div>
                <dt>tracked files</dt>
                <dd>{s.inventory.tracked_files.toLocaleString()}</dd>
              </div>
              <div>
                <dt>events inserted since boot</dt>
                <dd>{s.inventory.events_inserted_since_boot.toLocaleString()}</dd>
              </div>
              <div>
                <dt>parse errors since boot</dt>
                <dd>{s.inventory.parse_errors_since_boot.toLocaleString()}</dd>
              </div>
            </dl>
          </section>
          <section className="stats-strip__section" aria-label="Development nodes">
            <h3 className="stats-strip__section-title">Development node</h3>
            {primaryNode ? (
              <>
                <dl className="stats-strip__details">
                  <div>
                    <dt>name</dt>
                    <dd>{primaryNode.display_name}</dd>
                  </div>
                  <div>
                    <dt>connection</dt>
                    <dd>{nodeStatusLabel(primaryNode.connection_state)}</dd>
                  </div>
                  <div>
                    <dt>protocol</dt>
                    <dd>
                      {primaryNode.protocol_version == null
                        ? "unknown"
                        : `v${primaryNode.protocol_version}`}
                    </dd>
                  </div>
                  <div>
                    <dt>last heartbeat</dt>
                    <dd>{formatTimestamp(primaryNode.last_heartbeat_at)}</dd>
                  </div>
                  {primaryNode.pending_key_fingerprint && (
                    <div>
                      <dt>identity</dt>
                      <dd>{primaryNode.pending_key_fingerprint}</dd>
                    </div>
                  )}
                </dl>
                {primaryNode.pending_key_fingerprint && (
                  <button
                    type="button"
                    className="stats-strip__approve"
                    disabled={approvingNodeId === primaryNode.id}
                    onClick={approvePendingNode}
                    value={primaryNode.id}
                  >
                    {approvingNodeId === primaryNode.id
                      ? "Approving…"
                      : "Approve node"}
                  </button>
                )}
                {approvalError && (
                  <span className="stats-strip__approval-error" role="alert">
                    {approvalError}
                  </span>
                )}
              </>
            ) : (
              <span className="stats-strip__empty">No node paired</span>
            )}
          </section>
        </div>
      )}
    </div>
  );
}

function nodeStatusLabel(state: string): string {
  return state.replaceAll("_", " ");
}

function formatTimestamp(value: string | null): string {
  if (!value) return "never";
  return new Date(value).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
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
