import type {
  AppStateResponse,
  RepoView,
  SessionView,
  StatsResponse,
  WorkspaceView,
} from "../api/types";

export function statsPayload(overrides: Partial<StatsResponse> = {}): StatsResponse {
  return {
    uptime_seconds: 3_700,
    process: {
      memory_rss_bytes: 120 * 1024 * 1024,
      cpu_percent: 4.2,
      memory_limit_bytes: 500 * 1024 * 1024,
      ...overrides.process,
    },
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
  sessions = [],
  repos = [],
  workspaces = [],
  stats = statsPayload(),
}: {
  sessions?: SessionView[] | Array<Record<string, unknown>>;
  repos?: RepoView[] | Array<Record<string, unknown>>;
  workspaces?: WorkspaceView[] | Array<Record<string, unknown>>;
  stats?: StatsResponse;
} = {}): AppStateResponse {
  return {
    generated_at: "2026-05-02T00:00:00Z",
    sessions: sessions as SessionView[],
    repos: repos as RepoView[],
    workspaces: workspaces as WorkspaceView[],
    stats,
  };
}

export function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}
