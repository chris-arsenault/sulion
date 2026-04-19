// Mirrors the Rust SessionView in backend/src/routes.rs. Keep in sync
// manually — a small enough surface that codegen would be overkill.

export type SessionState = "live" | "dead" | "deleted" | "orphaned";

export interface SessionView {
  id: string;
  repo: string;
  working_dir: string;
  state: SessionState;
  created_at: string;
  ended_at: string | null;
  exit_code: number | null;
  current_claude_session_uuid: string | null;
  /** MAX(event.timestamp) for this session's current Claude UUID.
   * Null when no events have been ingested yet. Drives the sidebar
   * unread-dot indicator. */
  last_event_at: string | null;
  /** User-facing label; overrides the uuid prefix in the sidebar. */
  label: string | null;
  /** Pinned sessions float to the top of their repo group. */
  pinned: boolean;
  /** Palette-constrained colour tag name. */
  color: SessionColor | null;
}

export type SessionColor =
  | "amber"
  | "emerald"
  | "sky"
  | "rose"
  | "violet"
  | "slate"
  | "teal"
  | "fuchsia";

export const SESSION_COLORS: readonly SessionColor[] = [
  "amber",
  "emerald",
  "sky",
  "rose",
  "violet",
  "slate",
  "teal",
  "fuchsia",
] as const;

export interface UpdateSessionRequest {
  label?: string | null;
  pinned?: boolean;
  color?: SessionColor | null;
}

export interface ListSessionsResponse {
  sessions: SessionView[];
}

export interface CreateSessionRequest {
  repo: string;
  working_dir?: string;
  cols?: number;
  rows?: number;
  /** When set, the new shell boots straight into `claude --resume <uuid>`
   * and drops to an interactive bash after Claude exits. Used by the
   * Resume action on orphaned/ended sessions. */
  claude_resume_uuid?: string;
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

/** One canonical content block. Agent-agnostic: same shape whether
 * the source is Claude, Codex, or any future parser. `tool_name`
 * preserves the raw emitted name; `tool_name_canonical` is what the
 * renderers switch on. Unknown block kinds carry the original raw
 * JSON so the UI can show a placeholder without losing data. */
export interface TimelineBlock {
  ord: number;
  kind: "text" | "thinking" | "tool_use" | "tool_result" | "unknown";
  text?: string;
  tool_id?: string;
  tool_name?: string;
  tool_name_canonical?: string;
  tool_input?: unknown;
  is_error?: boolean;
  raw?: unknown;
}

export interface TimelineEvent {
  byte_offset: number;
  timestamp: string;
  kind: string;
  /** Raw JSONL payload. Kept for forensic use (and to cover events
   * that haven't been backfilled yet). New code should read `blocks`. */
  payload: unknown;
  /** Canonical content blocks, emitted by the ingester's parser. This
   * is the authoritative read path for renderers. Optional in the type
   * because test fixtures don't populate it; at runtime the backend
   * always emits an array (possibly empty). */
  blocks?: TimelineBlock[];
  /** Ingesting agent id — "claude-code", "codex", etc. */
  agent?: string;
  /** Normalised speaker: user / assistant / system / summary / other. */
  speaker?: string | null;
  /** Coarse content-kind discriminator for quick filtering without
   * walking `blocks`. */
  content_kind?: string | null;
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

export interface GitCommit {
  sha: string;
  subject: string;
  committed_at: string;
}

export interface GitStatus {
  branch: string | null;
  uncommitted_count: number;
  untracked_count: number;
  last_commit: GitCommit | null;
  recent_commits: GitCommit[];
  /** Repo-relative path → 2-char status code. */
  dirty_by_path: Record<string, string>;
}

export interface DirEntryView {
  name: string;
  kind: "file" | "dir";
  size: number;
  mtime: string | null;
  dirty: string | null;
}

export interface DirListing {
  path: string;
  entries: DirEntryView[];
}

export interface FileResponse {
  path: string;
  size: number;
  mime: string;
  binary: boolean;
  truncated: boolean;
  content: string | null;
}

export interface DiffResponse {
  diff: string;
}

export type SearchScope = "timeline" | "repo" | "workspace";

export type SearchHit =
  | {
      type: "file";
      repo: string;
      path: string;
      line: number;
      preview: string;
    }
  | {
      type: "event";
      session_id: string;
      claude_session_uuid: string;
      byte_offset: number;
      kind: string;
      timestamp: string;
      preview: string;
    }
  | { type: "done" }
  | { type: "error"; message: string };

export interface StatsResponse {
  uptime_seconds: number;
  process: {
    memory_rss_bytes: number;
    cpu_percent: number;
    memory_limit_bytes: number | null;
  };
  ingester: {
    files_seen_total: number;
    events_inserted_total: number;
    parse_errors_total: number;
  };
  pty: {
    tracked_sessions: number;
  };
  db: {
    database_size_bytes: number;
    events_rowcount: number;
    claude_sessions_rowcount: number;
    pty_sessions_rowcount: number;
    ingester_state_rowcount: number;
  };
}
