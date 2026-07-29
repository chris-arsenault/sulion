import type {
  AppStateResponse,
  NodeView,
  PlanSummaryView,
  RepoView,
  SessionView,
  StatsResponse,
  WorkspaceView,
} from "../api/types";

export function statsPayload(overrides: Partial<StatsResponse> = {}): StatsResponse {
  return {
    uptime_seconds: 3_700,
    // Not spread: `null` is a meaningful override here — it is what the
    // control plane reports while no node has sent a heartbeat.
    node:
      overrides.node === undefined
        ? {
            memory_used_bytes: 11 * 1024 * 1024 * 1024,
            memory_total_bytes: 32 * 1024 * 1024 * 1024,
            cpu_percent: 4.2,
          }
        : overrides.node,
    pty: {
      live_sessions: 3,
      live_agent_sessions: 2,
      ...overrides.pty,
    },
    db: {
      database_size_bytes: 50 * 1024 * 1024,
      ...overrides.db,
    },
    ingest: {
      last_tick_started_at_unix: null,
      last_progress_at_unix: null,
      stalled_seconds: null,
      ...overrides.ingest,
    },
    inventory: {
      event_rows: 1234,
      agent_sessions: 7,
      pty_sessions: 5,
      tracked_files: 7,
      events_inserted_since_boot: 250,
      parse_errors_since_boot: 0,
      ...overrides.inventory,
    },
  };
}

export function appStatePayload({
  nodes = [],
  sessions = [],
  repos = [],
  workspaces = [],
  stats = statsPayload(),
  plans = [],
}: {
  nodes?: NodeView[];
  sessions?: SessionView[] | Array<Record<string, unknown>>;
  repos?: RepoView[] | Array<Record<string, unknown>>;
  workspaces?: WorkspaceView[] | Array<Record<string, unknown>>;
  stats?: StatsResponse;
  plans?: PlanSummaryView[];
} = {}): AppStateResponse {
  return {
    generated_at: "2026-05-02T00:00:00Z",
    nodes,
    sessions: sessions as SessionView[],
    repos: repos as RepoView[],
    workspaces: workspaces as WorkspaceView[],
    plans,
    stats,
  };
}

export function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}
