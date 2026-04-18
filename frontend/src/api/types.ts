// Mirrors the Rust SessionView in backend/src/routes.rs. Keep in sync
// manually — a small enough surface that codegen would be overkill.

export type SessionState = "live" | "dead" | "deleted";

export interface SessionView {
  id: string;
  repo: string;
  working_dir: string;
  state: SessionState;
  created_at: string;
  ended_at: string | null;
  exit_code: number | null;
  current_claude_session_uuid: string | null;
}

export interface ListSessionsResponse {
  sessions: SessionView[];
}

export interface CreateSessionRequest {
  repo: string;
  working_dir?: string;
  cols?: number;
  rows?: number;
}

export interface RepoView {
  name: string;
  path: string;
}

export interface ListReposResponse {
  repos: RepoView[];
}

export interface CreateRepoRequest {
  name: string;
  git_url?: string;
}

export interface TimelineEvent {
  byte_offset: number;
  timestamp: string;
  kind: string;
  // Opaque JSONB — shape depends on `kind`. Tool-call renderers in #11
  // will narrow this further.
  payload: unknown;
}

export interface HistoryResponse {
  claude_session_uuid: string | null;
  events: TimelineEvent[];
  next_after: number | null;
}

export interface HistoryQuery {
  after?: number;
  limit?: number;
  kind?: string;
  claude_session?: string;
}
