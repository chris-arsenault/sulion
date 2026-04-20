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

export type OperationCategory =
  | "create_content"
  | "inspect"
  | "utility"
  | "research"
  | "delegate"
  | "workflow"
  | "other";

/** One canonical content block. Agent-agnostic: same shape whether
 * the source is Claude, Codex, or any future parser. `tool_name`
 * preserves the raw emitted name; `tool_name_canonical` is what the
 * renderers switch on, while `operation_category` is the coarser
 * app-facing grouping projected by the backend from ref-data rules.
 * The API intentionally omits any raw per-block JSON to force
 * consumers onto the canonical form. */
export interface TimelineBlock {
  ord: number;
  kind: "text" | "thinking" | "tool_use" | "tool_result" | "unknown";
  text?: string;
  tool_id?: string;
  tool_name?: string;
  tool_name_canonical?: string;
  operation_type?: string;
  operation_category?: OperationCategory;
  tool_input?: unknown;
  tool_output?: unknown;
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

export type SpeakerFacet = "user" | "assistant" | "tool_result";

export interface TimelineQuery {
  session?: string;
  hidden_speakers?: SpeakerFacet[];
  hidden_operation_categories?: OperationCategory[];
  errors_only?: boolean;
  show_bookkeeping?: boolean;
  show_sidechain?: boolean;
  file_path?: string;
}

export interface TimelineToolResult {
  content?: string | null;
  payload?: unknown | null;
  is_error: boolean;
}

export interface TimelineFileTouch {
  repo: string;
  path: string;
  touch_kind: string;
  is_write: boolean;
}

export interface TimelineSubagent {
  title: string;
  event_count: number;
  turns: TimelineTurn[];
}

export interface TimelineToolPair {
  id: string;
  name: string;
  raw_name?: string | null;
  operation_type?: string | null;
  category?: OperationCategory | null;
  input?: unknown;
  result?: TimelineToolResult | null;
  is_error: boolean;
  is_pending: boolean;
  file_touches: TimelineFileTouch[];
  subagent?: TimelineSubagent | null;
}

export type TimelineAssistantItem =
  | { kind: "text"; text: string }
  | { kind: "tool"; pair_id: string };

export type TimelineChunk =
  | { kind: "assistant"; items: TimelineAssistantItem[]; thinking: string[] }
  | { kind: "tool"; pair_id: string }
  | { kind: "summary"; subtype: string | null; text: string }
  | { kind: "system"; subtype: string | null; text: string; is_meta: boolean }
  | {
      kind: "generic";
      label: string;
      details: {
        event_uuid: string | null;
        parent_event_uuid: string | null;
        related_tool_use_id: string | null;
        subtype: string | null;
        speaker: string | null;
        content_kind: string | null;
        blocks: TimelineBlock[];
      };
    };

export interface TimelineTurn {
  id: number;
  turn_key?: string | null;
  preview: string;
  user_prompt_text?: string | null;
  start_timestamp: string;
  end_timestamp: string;
  duration_ms: number;
  event_count: number;
  operation_count: number;
  tool_pairs: TimelineToolPair[];
  thinking_count: number;
  has_errors: boolean;
  markdown: string;
  chunks: TimelineChunk[];
  pty_session_id?: string | null;
  session_uuid?: string | null;
  session_agent?: string | null;
  session_label?: string | null;
  session_state?: SessionState | null;
}

export interface TimelineResponse {
  session_uuid: string | null;
  session_agent: string | null;
  total_event_count: number;
  turns: TimelineTurn[];
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
  /** Repo-relative path → current working-copy churn. */
  diff_stats_by_path: Record<string, DiffStat>;
}

export interface DiffStat {
  additions: number;
  deletions: number;
}

export interface DirEntryView {
  name: string;
  kind: "file" | "dir";
  size: number;
  mtime: string | null;
  dirty: string | null;
  diff?: DiffStat | null;
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

export interface FileTraceTouch {
  pty_session_id: string | null;
  session_uuid: string;
  session_agent: string | null;
  session_label: string | null;
  session_state: SessionState | null;
  turn_id: number;
  turn_preview: string;
  turn_timestamp: string;
  operation_type: string | null;
  operation_category: OperationCategory | null;
  touch_kind: string;
  is_write: boolean;
}

export interface FileTraceResponse {
  path: string;
  dirty: string | null;
  current_diff: DiffStat | null;
  touches: FileTraceTouch[];
}

export interface StatsResponse {
  uptime_seconds: number;
  process: {
    memory_rss_bytes: number;
    cpu_percent: number;
    memory_limit_bytes: number | null;
  };
  pty: {
    live_sessions: number;
    live_agent_sessions: number;
  };
  db: {
    database_size_bytes: number;
  };
  inventory: {
    event_rows: number;
    agent_sessions: number;
    pty_sessions: number;
    tracked_files: number;
    files_seen_since_boot: number;
    events_inserted_since_boot: number;
    parse_errors_since_boot: number;
  };
}

/** One global library entry. References store assistant output for
 * later rereading; prompts store reusable user instructions. */
export interface LibraryEntry {
  slug: string;
  name: string;
  created_at: string | null;
  updated_at: string | null;
  body: string;
}

export type LibraryKind = "references" | "prompts";

export interface SaveLibraryInput {
  name: string;
  body: string;
}

export type FuturePromptState = "pending" | "sent";

export interface FuturePromptEntry {
  id: string;
  state: FuturePromptState;
  created_at: string | null;
  updated_at: string | null;
  text: string;
}

export interface FuturePromptListResponse {
  session_uuid: string | null;
  session_agent: string | null;
  prompts: FuturePromptEntry[];
}

export interface CreateFuturePromptInput {
  text: string;
}

export interface UpdateFuturePromptInput {
  text?: string;
  state?: FuturePromptState;
}
