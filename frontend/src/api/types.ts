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
  current_session_uuid: string | null;
  current_session_agent: string | null;
  /** MAX(event.timestamp) for this session's current transcript session.
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
  /** Resume session id when the backend supports agent-specific resume. */
  resume_session_uuid?: string;
  /** Agent id for `resume_session_uuid`. */
  resume_agent?: string;
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
 * renderers switch on. The API intentionally omits any raw per-block
 * JSON to force consumers onto the canonical form. */
export interface TimelineBlock {
  ord: number;
  kind: "text" | "thinking" | "tool_use" | "tool_result" | "unknown";
  text?: string;
  tool_id?: string;
  tool_name?: string;
  tool_name_canonical?: string;
  tool_input?: unknown;
  is_error?: boolean;
}

export interface TimelineEvent {
  byte_offset: number;
  timestamp: string;
  kind: string;
  /** Ingesting agent id — "claude-code", "codex", etc. */
  agent: string;
  /** Normalised speaker: user / assistant / system / summary / other. */
  speaker: string | null;
  /** Coarse content-kind discriminator for quick filtering without
   * walking `blocks`. */
  content_kind: string | null;
  /** Stable event id emitted by the source transcript, when present. */
  event_uuid: string | null;
  /** Parent event id for sidechain/subagent lineage, when present. */
  parent_event_uuid: string | null;
  /** Related tool_use id carried by some result/report rows. */
  related_tool_use_id: string | null;
  /** True when this event belongs to a Task-subagent conversation. */
  is_sidechain: boolean;
  /** True for internal/bookkeeping system events. */
  is_meta: boolean;
  /** Optional subtype for system/bookkeeping rows. */
  subtype: string | null;
  /** Canonical content blocks, emitted by the ingester's parser. */
  blocks: TimelineBlock[];
}

export interface HistoryResponse {
  session_uuid: string | null;
  session_agent: string | null;
  events: TimelineEvent[];
  next_after: number | null;
}

export interface HistoryQuery {
  after?: number;
  limit?: number;
  kind?: string;
  session?: string;
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
      session_uuid: string;
      session_agent: string;
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
    agent_sessions_rowcount: number;
    pty_sessions_rowcount: number;
    ingester_state_rowcount: number;
  };
}

/** Per-repo library entry (refs or prompts). Frontmatter is a thin
 * markdown header; the body is plain text. */
export interface LibraryEntry {
  slug: string;
  name: string;
  tags: string[];
  created_at: string | null;
  body: string;
  /** Any additional frontmatter keys the backend didn't recognise. */
  extras: Record<string, unknown>;
}

export type LibraryKind = "refs" | "prompts";

export interface SaveLibraryInput {
  name: string;
  tags?: string[];
  body: string;
}
